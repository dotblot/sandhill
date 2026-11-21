//! Inline review and progress for changes to R, Python and their packages.

use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::Disableable;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use sandhill_proto::{
    Call, CopyReport, CopyRow, EnvAction, EnvActionPlan, FailureFix, FileText, InstallFailure,
    InstallFailureKind, InstallPrefs, InstallTarget, InstallTool, InstallToolInfo, NewRuntime,
    OwnSetupPlan, PackageAction, PackageChange, PackagePlan, ProjectFilePlan, ProjectSetupPlan,
    RuntimePlan, TerminalInfo,
};

use crate::daemon::Daemon;
use crate::look::{self, Palette};

pub(crate) enum Review<T> {
    Planning,
    Ready(T),
    Error(String),
}

impl<T, E: std::fmt::Display> From<Result<T, E>> for Review<T> {
    fn from(result: Result<T, E>) -> Self {
        match result {
            Ok(plan) => Self::Ready(plan),
            Err(error) => Self::Error(format!("{error:#}")),
        }
    }
}

pub(crate) enum Choice {
    Tool(InstallTool),
    Folder(String),
}

impl Choice {
    pub(crate) fn apply(self, change: &mut PackageChange) {
        match self {
            Self::Tool(tool) => change.tool = Some(tool),
            Self::Folder(folder) => {
                if let InstallTarget::R { library, .. } = &mut change.target {
                    *library = folder;
                }
            }
        }
    }
}

/// Use a prior library only if this is the same R and the folder is still a choice.
pub(crate) fn r_target(r: String, libraries: &[String], prefs: &InstallPrefs) -> InstallTarget {
    let library = prefs
        .last_for("r")
        .and_then(|last| match &last.target {
            InstallTarget::R { r: old, library } if *old == r && libraries.contains(library) => {
                Some(library)
            }
            _ => None,
        })
        .or_else(|| libraries.first())
        .cloned()
        .unwrap_or_default();
    InstallTarget::R { r, library }
}

/// Never carry unattended approval over to a different target or tool.
pub(crate) fn automatic_tool(change: &PackageChange, prefs: &InstallPrefs) -> Option<InstallTool> {
    if !prefs.without_asking || change.action != PackageAction::Install {
        return None;
    }
    prefs
        .last_for(change.target.language())
        .filter(|last| last.target == change.target)
        .map(|last| last.tool)
}

pub(crate) fn tool_name(tool: &InstallToolInfo) -> &str {
    if tool.tool == InstallTool::Conda {
        std::path::Path::new(&tool.path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(tool.tool.label())
    } else {
        tool.tool.label()
    }
}

fn package_tool(plan: &PackagePlan) -> &str {
    plan.change.tool.map_or("", |selected| {
        plan.tools
            .iter()
            .find(|info| info.tool == selected)
            .map_or(selected.label(), tool_name)
    })
}

pub(crate) fn names(items: &[String]) -> String {
    match items.split_last() {
        Some((last, [])) => last.clone(),
        Some((last, before)) => format!("{} and {last}", before.join(", ")),
        None => "packages".into(),
    }
}

pub(crate) fn auto_notice(plan: &PackagePlan) -> String {
    format!(
        "Installed {} in {} with {}.",
        names(&plan.change.packages),
        plan.folder,
        package_tool(plan),
    )
}

pub(crate) fn verb(action: PackageAction) -> &'static str {
    match action {
        PackageAction::Install => "Install",
        PackageAction::Update => "Update",
        PackageAction::Remove => "Remove",
    }
}

pub(crate) fn progress_title(change: &PackageChange) -> String {
    let action = match change.action {
        PackageAction::Install => "Installing",
        PackageAction::Update => "Updating",
        PackageAction::Remove => "Removing",
    };
    format!("{action} {}", names(&change.packages))
}

pub(crate) fn runtime_name(runtime: &NewRuntime) -> String {
    match runtime {
        NewRuntime::RFromCran => "R from CRAN".into(),
        NewRuntime::RFromHomebrew => "R from Homebrew".into(),
        NewRuntime::RConda { .. } => "R and IRkernel in a conda environment".into(),
        NewRuntime::PythonConda { python, .. } | NewRuntime::PythonUv { python, .. } => {
            format!("Python {python} and ipykernel")
        }
        NewRuntime::Miniforge => "Miniforge".into(),
    }
}

pub(crate) fn own_setup(
    review: &Review<sandhill_proto::OwnSetupPlan>,
    expanded: bool,
    details: Button,
    developer_tools: Div,
    space: Button,
    p: &Palette,
) -> Div {
    let plan = match review {
        Review::Planning => return body().child(line("Working out the setup…", p.muted)),
        Review::Error(error) => return body().child(line(error.clone(), p.err)),
        Review::Ready(plan) => plan,
    };
    let what = match (plan.setup.r, plan.setup.python) {
        (true, true) => "R and Python",
        (true, false) => "R",
        (false, true) => "Python",
        (false, false) => "No languages selected",
    };
    let summary = if plan.steps.is_empty() && plan.copy.is_empty() && plan.refused.is_none() {
        "Everything selected is already installed.".into()
    } else {
        format!("Set up {what} and the selected packages in one folder.")
    };
    body()
        .child(line(summary, p.text).font_weight(FontWeight::MEDIUM))
        .child(line(format!("Folder: {}", plan.setup.folder), p.muted))
        .when(plan.refused.is_none(), |d| {
            d.child(line(
                format!(
                    "About {} MB to download · {} MB free · about {} {}",
                    plan.download_mb,
                    plan.free_mb,
                    plan.minutes,
                    if plan.minutes == 1 {
                        "minute"
                    } else {
                        "minutes"
                    },
                ),
                p.muted,
            ))
        })
        .when_some(plan.refused.as_ref(), |d, reason| {
            d.child(line(reason.clone(), p.err))
                .children(space_for(&plan.refused_fix, space))
        })
        .children(plan.notes.iter().map(|note| line(note.clone(), p.muted)))
        .when(plan.needs_developer_tools, |d| d.child(developer_tools))
        .when(!plan.steps.is_empty(), |d| {
            d.child(h_flex().child(details.label(if expanded {
                "Hide Details"
            } else {
                "Details"
            })))
            .children(plan.steps.iter().enumerate().map(|(ix, step)| {
                v_flex()
                    .min_w_0()
                    .gap(px(look::GAP_4))
                    .child(line(step.what.clone(), p.text))
                    .when(expanded && !step.command.is_empty(), |d| {
                        d.child(command_with_copy(ix, &step.command, p))
                    })
            }))
        })
        .when(expanded, |d| {
            d.children(
                [
                    ("R packages", &plan.r_packages),
                    ("Python packages", &plan.python_packages),
                ]
                .into_iter()
                .filter(|(_, packages)| !packages.is_empty())
                .map(|(title, packages)| {
                    v_flex().child(line(title, p.text)).child(line(
                        packages
                            .iter()
                            .map(|item| format!("{} ({})", item.name, item.source))
                            .collect::<Vec<_>>()
                            .join(", "),
                        p.muted,
                    ))
                }),
            )
        })
}

fn copy_summary(rows: &[CopyRow]) -> String {
    let mut counts = [0; 7];
    for row in rows {
        let ix = match (row.cant.as_ref(), row.source.as_deref(), row.tool) {
            (Some(_), _, _) | (_, None, _) => 6,
            (_, _, Some(InstallTool::Conda)) | (_, Some("conda-forge" | "bioconda"), _) => {
                usize::from(row.new_version.as_ref().is_some_and(|v| v != &row.version))
            }
            (_, Some("CRAN" | "Bioconductor"), _) => 2,
            (_, Some("PyPI"), _) => 3,
            (_, Some("GitHub"), _) => 4,
            _ => 5,
        };
        counts[ix] += 1;
    }
    counts
        .into_iter()
        .zip([
            "from conda, same version",
            "from conda, another version",
            "from CRAN or Bioconductor, built here",
            "from PyPI",
            "from GitHub",
            "from another source",
            "can't be copied",
        ])
        .filter(|(count, _)| *count > 0)
        .map(|(count, label)| format!("{count} {label}"))
        .collect::<Vec<_>>()
        .join("; ")
}

pub(crate) fn own_copy(
    review: &Review<OwnSetupPlan>,
    expanded: bool,
    details: Button,
    p: &Palette,
) -> Option<Div> {
    let Review::Ready(plan) = review else {
        return None;
    };
    if plan.setup.copy_from.is_empty() || plan.refused.is_some() {
        return None;
    }
    let panel = body()
        .border_t_1()
        .border_color(p.line)
        .child(line("Where the copies will come from", p.text).font_weight(FontWeight::MEDIUM));
    if plan.copy.is_empty() {
        return Some(panel.child(line("No additional packages to copy.", p.muted)));
    }
    let summary = format!("{}.", copy_summary(&plan.copy));
    let row = |cells: [Div; 4]| {
        let [name, current, source, version] = cells;
        h_flex()
            .w_full()
            .items_start()
            .gap(px(look::GAP_8))
            .py(px(look::GAP_4))
            .child(name.w(relative(0.24)))
            .child(current.w(relative(0.18)))
            .child(source.w(relative(0.22)))
            .child(version.flex_1())
    };
    Some(
        panel
            .child(line(summary, p.muted))
            .child(h_flex().child(details.label(if expanded {
                "Hide Copy Table"
            } else {
                "Show Copy Table"
            })))
            .when(expanded, |d| {
                d.child(row([
                    "Package",
                    "Your version",
                    "Copy from",
                    "New version or reason",
                ]
                .map(|heading| {
                    line(heading, p.text).font_weight(FontWeight::MEDIUM)
                })))
                .child(
                    div()
                        .id("setup-copy-table")
                        .max_h(px(look::row() * 9.))
                        .overflow_y_scroll()
                        .children(plan.copy.iter().map(|item| {
                            let language = if item.language == "r" { "R" } else { "Python" };
                            let name = format!("{} ({language})", item.name);
                            let current = format!("{}\n{}", item.version, item.from);
                            let source = item.source.as_deref().unwrap_or("Can't be copied");
                            let version = item
                                .cant
                                .as_deref()
                                .or(item.new_version.as_deref())
                                .unwrap_or("Not known yet");
                            row([
                                line(name, p.text),
                                line(current, p.muted),
                                line(source.to_owned(), p.muted),
                                line(
                                    version.to_owned(),
                                    if item.cant.is_some() { p.warn } else { p.muted },
                                ),
                            ])
                            .border_t_1()
                            .border_color(p.line)
                        })),
                )
            }),
    )
}

pub(crate) fn copy_report<T: 'static>(
    report: &CopyReport,
    busy: bool,
    fix: impl Fn(&mut T, PackageChange, &mut Context<T>) + Copy + 'static,
    p: &Palette,
    cx: &mut Context<T>,
) -> Div {
    let count = report.failed.len();
    let copied = report.copied;
    let packages = if copied == 1 { "package" } else { "packages" };
    let summary = if count == 0 {
        format!("Copied {copied} {packages}.")
    } else {
        format!("Copied {copied} {packages}. {count} didn't copy:")
    };
    body().child(line(summary, p.text)).when(count > 0, |d| {
        d.child(
            div()
                .id("setup-copy-report")
                .max_h(px(look::row() * 9.))
                .overflow_y_scroll()
                .children(report.failed.iter().enumerate().map(|(ix, problem)| {
                    look::settings_row(
                        problem.package.clone(),
                        Some(look::settings_detail(problem.reason.clone(), p)),
                    )
                    .pl(px(0.))
                    .border_t_1()
                    .border_color(p.line)
                    .when_some(problem.fix.clone(), |row, change| {
                        row.child(
                            Button::new(("setup-copy-fix", ix))
                                .small()
                                .label("Repair…")
                                .disabled(busy)
                                .on_click(
                                    cx.listener(move |this, _, _, cx| {
                                        fix(this, change.clone(), cx)
                                    }),
                                ),
                        )
                    })
                })),
        )
    })
}

/// The first-run review uses the same package details as every other install.
pub(crate) fn setup_packages<T: 'static>(
    reviews: &[Review<PackagePlan>],
    libraries: &[String],
    space: Button,
    pick: impl Fn(&mut T, usize, Choice, &mut Context<T>) + Copy + 'static,
    p: &Palette,
    cx: &mut Context<T>,
) -> Div {
    let mut space = Some(space);
    body().children(reviews.iter().enumerate().map(|(ix, review)| match review {
        Review::Planning => line("Working out the command…", p.muted),
        Review::Ready(plan) => {
            let space = space.take_if(|_| needs_space(&plan.refused_fix));
            package_plan(ix, plan, libraries, space, &pick, p, cx)
        }
        Review::Error(error) => line(error.clone(), p.err),
    }))
}

fn line(text: impl Into<SharedString>, color: Hsla) -> Div {
    let text: SharedString = text.into();
    div()
        .min_w_0()
        .whitespace_normal()
        .text_color(color)
        .child(text)
}

fn body() -> Div {
    v_flex()
        .min_w_0()
        .gap(px(look::GAP_6))
        .px(px(look::PANE_PAD_X))
        .py(px(look::PANE_PAD_Y))
        .text_size(px(look::small()))
}

pub(crate) fn command_block(command: &str, p: &Palette) -> Div {
    command_with_copy(0, command, p)
}

/// The exact command in the code font, with Copy; `ix` tells several apart.
fn command_with_copy(ix: usize, command: &str, p: &Palette) -> Div {
    let copied = command.to_owned();
    v_flex()
        .min_w_0()
        .gap(px(look::GAP_4))
        .child(
            h_flex()
                .justify_between()
                .child(line("Command", p.muted))
                .child(
                    Button::new(("install-copy-command", ix))
                        .small()
                        .label("Copy")
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(copied.clone()));
                        }),
                ),
        )
        .child(
            div()
                .min_w_0()
                .p(px(look::GAP_8))
                .rounded(px(look::CONTROL_RADIUS))
                .bg(p.ink(look::ROW_HOVER))
                .font_family(look::mono())
                .text_size(px(look::small()))
                .whitespace_normal()
                .child(command.to_owned()),
        )
}

/// What runs, or why it can't, for one plan, with Free Up Space when the
/// disk is too full for it.
fn command_or_refusal(
    ix: usize,
    command: &str,
    refused: Option<&str>,
    space: Option<Button>,
    p: &Palette,
) -> Div {
    v_flex()
        .min_w_0()
        .gap(px(look::GAP_6))
        .when_some(refused, |d, text| d.child(line(text.to_owned(), p.err)))
        .when(refused.is_some(), |d| {
            d.children(space.map(|button| h_flex().child(button)))
        })
        .when(refused.is_none() && !command.is_empty(), |d| {
            d.child(command_with_copy(ix, command, p))
        })
}

/// Free Up Space, shown beside a refusal because the disk is too full;
/// the caller's click opens Resources and Storage, as for failures.
pub(crate) fn space_button() -> Button {
    Button::new("install-free-space")
        .small()
        .label("Free Up Space")
}

fn needs_space(fix: &Option<FailureFix>) -> bool {
    matches!(fix, Some(FailureFix::FreeUpSpace))
}

fn space_for(fix: &Option<FailureFix>, space: Button) -> Option<Div> {
    needs_space(fix).then(|| h_flex().child(space))
}

/// Cancel, then the verb when there is something to run; Remove is red.
fn buttons(action: PackageAction, runnable: bool, (cancel, run): (Button, Button)) -> Div {
    h_flex()
        .justify_end()
        .gap(px(look::GAP_8))
        .child(cancel)
        .when(runnable, |d| {
            d.child(if action == PackageAction::Remove {
                run.label(verb(action)).danger()
            } else {
                run.label(verb(action)).primary()
            })
        })
}

pub(crate) fn runnable(plan: &PackagePlan) -> bool {
    plan.refused.is_none() && !plan.command.is_empty()
}

fn status_row(message: Div, cancel: Button) -> Div {
    body()
        .child(message)
        .child(h_flex().justify_end().child(cancel))
}

/// One plan: the sentence, why, each package, the tool and folder
/// choices, notes and the command. `ix` is its place among several.
fn package_plan<T: 'static>(
    ix: usize,
    plan: &PackagePlan,
    libraries: &[String],
    space: Option<Button>,
    pick: &(impl Fn(&mut T, usize, Choice, &mut Context<T>) + Clone + 'static),
    p: &Palette,
    cx: &mut Context<T>,
) -> Div {
    let change = &plan.change;
    let verb = verb(change.action);
    let what = names(&change.packages);
    let summary = if plan.folder.is_empty() {
        format!("{verb} {what}.")
    } else {
        let direction = match change.action {
            PackageAction::Install => "into",
            PackageAction::Update => "in",
            PackageAction::Remove => "from",
        };
        format!("{verb} {what} {direction} {}.", plan.folder)
    };
    let mut panel = v_flex()
        .min_w_0()
        .gap(px(look::GAP_6))
        .child(line(summary, p.text).font_weight(FontWeight::MEDIUM))
        .when(!change.reason.is_empty(), |d| {
            d.child(line(format!("Why: {}.", change.reason), p.muted))
        })
        .children(
            plan.packages
                .iter()
                .map(|item| pinned_package(&item.name, item.version.as_deref(), &item.source, p)),
        );
    if plan.tools.len() > 1 {
        panel = panel.child(line("Tool", p.muted)).child(
            h_flex()
                .flex_wrap()
                .gap(px(look::GAP_4))
                .children(plan.tools.iter().map(|info| {
                    let tool = info.tool;
                    let pick = pick.clone();
                    Button::new(SharedString::from(format!(
                        "install-tool-{ix}-{}",
                        tool.id()
                    )))
                    .small()
                    .label(tool_name(info).to_owned())
                    .when(change.tool == Some(tool), |b| b.primary())
                    .on_click(
                        cx.listener(move |this, _, _, cx| pick(this, ix, Choice::Tool(tool), cx)),
                    )
                })),
        );
    } else if change.tool.is_some() {
        panel = panel.child(line(format!("Tool: {}", package_tool(plan)), p.muted));
    }
    if let InstallTarget::R { library, .. } = &change.target
        && change.tool != Some(InstallTool::Conda)
        && libraries.len() > 1
    {
        panel = panel
            .child(line("Folder", p.muted))
            .children(libraries.iter().map(|folder| {
                let chosen = folder == library;
                let folder = folder.clone();
                let pick = pick.clone();
                Button::new(SharedString::from(format!("install-folder-{ix}-{folder}")))
                    .small()
                    .label(folder.clone())
                    .when(chosen, |b| b.primary())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        pick(this, ix, Choice::Folder(folder.clone()), cx);
                    }))
            }));
    }
    panel
        .children(plan.notes.iter().map(|note| line(note.clone(), p.muted)))
        .child(command_or_refusal(
            ix,
            &plan.command,
            plan.refused.as_deref(),
            space,
            p,
        ))
        .when(ix > 0, |d| {
            d.pt(px(look::GAP_8))
                .border_t(px(0.5))
                .border_color(p.glass_edge)
        })
}

/// One confirmation for one or more plans (an R update gets one per
/// library), with what was left out (`skipped`) in plain words. Folder
/// choices from `libraries` apply to installs only. The verb runs the
/// plans that can run.
#[allow(clippy::too_many_arguments)]
pub(crate) fn packages<T: 'static>(
    reviews: &[Review<PackagePlan>],
    skipped: &[String],
    libraries: &[String],
    (cancel, run): (Button, Button),
    space: Button,
    pick: impl Fn(&mut T, usize, Choice, &mut Context<T>) + Clone + 'static,
    p: &Palette,
    cx: &mut Context<T>,
) -> Div {
    if reviews.iter().any(|r| matches!(r, Review::Planning)) {
        return status_row(line("Working out the command…", p.muted), cancel);
    }
    let failed = reviews.iter().find_map(|r| match r {
        Review::Error(err) => Some(err.clone()),
        _ => None,
    });
    let plans: Vec<&PackagePlan> = reviews
        .iter()
        .filter_map(|r| match r {
            Review::Ready(plan) => Some(plan),
            _ => None,
        })
        .collect();
    let skipped = skipped.iter().map(|text| line(text.clone(), p.muted));
    if let Some(err) = failed {
        return body()
            .child(line(err, p.err))
            .children(skipped)
            .child(h_flex().justify_end().child(cancel));
    }
    let action = plans
        .first()
        .map_or(PackageAction::Install, |plan| plan.change.action);
    let choices = if action == PackageAction::Install {
        libraries
    } else {
        &[]
    };
    let any = plans.iter().any(|plan| runnable(plan));
    let mut space = Some(space);
    let mut panel = body();
    for (ix, plan) in plans.into_iter().enumerate() {
        let space = space.take_if(|_| needs_space(&plan.refused_fix));
        panel = panel.child(package_plan(ix, plan, choices, space, &pick, p, cx));
    }
    panel
        .children(skipped)
        .child(buttons(action, any, (cancel, run)))
}

pub(crate) fn runtime(
    review: &Review<RuntimePlan>,
    cancel: Button,
    run: Button,
    space: Button,
    p: &Palette,
) -> Div {
    let plan = match review {
        Review::Planning => return status_row(line("Working out the command…", p.muted), cancel),
        Review::Error(err) => return status_row(line(err.clone(), p.err), cancel),
        Review::Ready(plan) => plan,
    };
    let what = runtime_name(&plan.runtime);
    let summary = if plan.folder.is_empty() {
        format!("Install {what}.")
    } else {
        format!("Install {what} into {}.", plan.folder)
    };
    let packages = match &plan.runtime {
        NewRuntime::PythonConda { sets, .. } | NewRuntime::PythonUv { sets, .. } => {
            sandhill_proto::PYTHON_SETS
                .iter()
                .filter(|(id, ..)| sets.iter().any(|set| set == id))
                .flat_map(|(_, _, packages)| packages.iter().copied())
                .collect::<Vec<_>>()
        }
        _ => Vec::new(),
    };
    let can_run = plan.refused.is_none() && !plan.command.is_empty();
    body()
        .child(line(summary, p.text).font_weight(FontWeight::MEDIUM))
        .when(!plan.tool.is_empty(), |d| {
            d.child(line(format!("Tool: {}", plan.tool), p.muted))
        })
        .when(!packages.is_empty(), |d| {
            d.child(line(
                format!("Also installs {}.", packages.join(", ")),
                p.muted,
            ))
        })
        .when_some(plan.download_mb, |d, mb| {
            d.child(line(format!("About {mb} MB to download."), p.muted))
        })
        .children(plan.notes.iter().map(|note| line(note.clone(), p.muted)))
        .child(command_or_refusal(
            0,
            &plan.command,
            plan.refused.as_deref(),
            needs_space(&plan.refused_fix).then_some(space),
            p,
        ))
        .child(buttons(PackageAction::Install, can_run, (cancel, run)))
}

pub(crate) fn env_job_title(action: &EnvAction) -> &'static str {
    match action {
        EnvAction::Check { .. } => "Checking the environment",
        EnvAction::Undo { .. } => "Undoing the last change",
        EnvAction::Duplicate { .. } => "Duplicating the environment",
        EnvAction::Export { .. } => "Exporting the package list",
        EnvAction::Import { .. } => "Importing the package list",
        EnvAction::Delete { .. } => "Deleting the environment",
        EnvAction::ReinstallFromOldR { .. } => "Reinstalling the packages from the earlier R",
    }
}

pub(crate) fn environment(
    review: &Review<EnvActionPlan>,
    name: &str,
    (cancel, run): (Button, Button),
    space: Button,
    p: &Palette,
) -> Div {
    let plan = match review {
        Review::Planning => return status_row(line("Working out the command…", p.muted), cancel),
        Review::Error(err) => return status_row(line(err.clone(), p.err), cancel),
        Review::Ready(plan) => plan,
    };
    let (label, delete) = match plan.action {
        EnvAction::Check { .. } => ("Check", false),
        EnvAction::Undo { .. } => ("Undo", false),
        EnvAction::Duplicate { .. } => ("Duplicate", false),
        EnvAction::Export { .. } => ("Export", false),
        EnvAction::Import { .. } => ("Import", false),
        EnvAction::Delete { .. } => ("Delete", true),
        EnvAction::ReinstallFromOldR { .. } => ("Reinstall", false),
    };
    body()
        .when(delete, |d| {
            d.child(line(format!("Delete {name}?"), p.text).font_weight(FontWeight::SEMIBOLD))
        })
        .child(line(plan.what.clone(), p.text).font_weight(FontWeight::MEDIUM))
        .when(!plan.folder.is_empty(), |d| {
            d.child(line(format!("Folder: {}", plan.folder), p.muted))
        })
        .children(plan.notes.iter().map(|note| line(note.clone(), p.muted)))
        .child(command_or_refusal(
            0,
            &plan.command,
            plan.refused.as_deref(),
            needs_space(&plan.refused_fix).then_some(space),
            p,
        ))
        .child(
            h_flex()
                .justify_end()
                .gap(px(look::GAP_8))
                .child(cancel)
                .when(plan.refused.is_none() && !plan.command.is_empty(), |d| {
                    d.child(if delete {
                        run.label(label).danger()
                    } else {
                        run.label(label).primary()
                    })
                }),
        )
}

fn project_review_buttons(cancel: Button, run: Option<Button>) -> Div {
    h_flex()
        .justify_end()
        .gap(px(look::GAP_8))
        .child(cancel)
        .children(run.map(|b| b.primary()))
}

fn pinned_package(name: &str, version: Option<&str>, source: &str, p: &Palette) -> Div {
    let mut text = name.to_owned();
    if let Some(version) = version.filter(|version| !version.is_empty()) {
        text.push(' ');
        text.push_str(version);
    }
    if !source.is_empty() {
        text.push_str(" (");
        text.push_str(source);
        text.push(')');
    }
    line(text, p.muted)
}

pub(crate) fn project_file(
    review: &Review<ProjectFilePlan>,
    expanded: bool,
    contents: Button,
    (cancel, save): (Button, Button),
    p: &Palette,
) -> Div {
    let plan = match review {
        Review::Planning => return status_row(line("Working out what to save…", p.muted), cancel),
        Review::Error(error) => return status_row(line(error.clone(), p.err), cancel),
        Review::Ready(plan) => plan,
    };
    body()
        .when(!plan.path.is_empty(), |d| {
            d.child(line(format!("File: {}", plan.path), p.text).font_weight(FontWeight::MEDIUM))
        })
        .when(plan.replaces, |d| {
            d.child(line(
                "Saving replaces the file that's already there.",
                p.warn,
            ))
        })
        .children(
            plan.refused
                .as_ref()
                .map(|reason| line(reason.clone(), p.err)),
        )
        .children(plan.notes.iter().map(|note| line(note.clone(), p.muted)))
        .when(!plan.packages.is_empty(), |d| {
            d.child(line("Packages to save", p.text)).child(
                div()
                    .id("project-file-packages")
                    .max_h(px(look::row() * 6.))
                    .overflow_y_scroll()
                    .children(plan.packages.iter().map(|item| {
                        pinned_package(&item.name, Some(&item.version), &item.source, p)
                    })),
            )
        })
        .when(!plan.contents.is_empty(), |d| {
            d.child(h_flex().child(contents.label(if expanded {
                "Hide File Contents"
            } else {
                "Show File Contents"
            })))
            .when(expanded, |d| {
                d.child(
                    div()
                        .id("project-file-contents")
                        .max_h(px(look::row() * 8.))
                        .overflow_y_scroll()
                        .p(px(look::GAP_8))
                        .rounded(px(look::CONTROL_RADIUS))
                        .bg(p.ink(look::ROW_HOVER))
                        .font_family(look::mono())
                        .child(line(plan.contents.clone(), p.text)),
                )
            })
        })
        .child(project_review_buttons(
            cancel,
            plan.refused.is_none().then(|| save.label("Save")),
        ))
}

pub(crate) fn project_setup(
    review: &Review<ProjectSetupPlan>,
    (cancel, run): (Button, Button),
    space: Button,
    p: &Palette,
) -> Div {
    let plan = match review {
        Review::Planning => return status_row(line("Working out the setup…", p.muted), cancel),
        Review::Error(error) => return status_row(line(error.clone(), p.err), cancel),
        Review::Ready(plan) => plan,
    };
    body()
        .child(line(plan.what.clone(), p.text).font_weight(FontWeight::MEDIUM))
        .when(!plan.folder.is_empty(), |d| {
            d.child(line(format!("Folder: {}", plan.folder), p.muted))
        })
        .when_some(plan.download_mb, |d, mb| {
            d.child(line(format!("About {mb} MB to download"), p.muted))
        })
        .children(
            plan.refused
                .as_ref()
                .map(|reason| line(reason.clone(), p.err)),
        )
        .when(plan.refused.is_some(), |d| {
            d.children(space_for(&plan.refused_fix, space))
        })
        .children(plan.notes.iter().map(|note| line(note.clone(), p.muted)))
        .when(!plan.packages.is_empty(), |d| {
            d.child(line("Packages in the file", p.text)).child(
                div()
                    .id("project-setup-packages")
                    .max_h(px(look::row() * 6.))
                    .overflow_y_scroll()
                    .children(plan.packages.iter().map(|item| {
                        pinned_package(&item.name, item.version.as_deref(), &item.source, p)
                    })),
            )
        })
        .children(plan.steps.iter().enumerate().map(|(ix, step)| {
            v_flex()
                .min_w_0()
                .gap(px(look::GAP_4))
                .child(line(step.what.clone(), p.text))
                .when(!step.command.is_empty(), |d| {
                    d.child(command_with_copy(ix, &step.command, p))
                })
        }))
        .child(project_review_buttons(
            cancel,
            (plan.refused.is_none() && !plan.steps.is_empty()).then(|| run.label("Set Up")),
        ))
}

pub(crate) fn cancel_button() -> Button {
    Button::new("install-review-cancel").small().label("Cancel")
}

pub(crate) fn run_button() -> Button {
    Button::new("install-review-run").small()
}

/// Installs can finish in Welcome or in another project window on this device.
#[derive(Default)]
pub(crate) struct InstallCompletions(HashMap<String, u64>);
impl Global for InstallCompletions {}

impl InstallCompletions {
    pub(crate) fn count(host: &str, cx: &App) -> u64 {
        cx.try_global::<Self>()
            .and_then(|state| state.0.get(host))
            .copied()
            .unwrap_or_default()
    }

    pub(crate) fn finished(host: String, cx: &mut App) {
        *cx.default_global::<Self>().0.entry(host).or_default() += 1;
    }
}

pub(crate) struct JobStatus {
    output: Option<String>,
    finished: Option<bool>,
    failure: Option<InstallFailure>,
}

enum DeveloperTools {
    Opening,
    Opened,
    Failed,
}

pub(crate) struct Job {
    pub(crate) id: String,
    pub(crate) log: Option<String>,
    pub(crate) title: String,
    output: SharedString,
    pub(crate) finished: Option<bool>,
    pub(crate) details: bool,
    pub(crate) change: Option<PackageChange>,
    failure: Option<InstallFailure>,
    developer_tools: Option<DeveloperTools>,
}

impl Job {
    pub(crate) fn new(info: TerminalInfo, log: Option<String>, title: String) -> Self {
        Self {
            finished: (!info.running).then(|| info.exit.is_some_and(|e| e.code == Some(0))),
            id: info.id,
            log,
            title,
            output: SharedString::default(),
            details: false,
            change: None,
            failure: None,
            developer_tools: None,
        }
    }

    pub(crate) fn update(&mut self, status: JobStatus) {
        if let Some(output) = status.output {
            self.output = output.into();
        }
        self.finished = status.finished;
        self.failure = status.failure;
    }

    pub(crate) fn alternative(
        &self,
        tool: InstallTool,
        proposed: Option<PackageChange>,
    ) -> Option<PackageChange> {
        let mut change = match proposed {
            Some(change) => change,
            None => self
                .change
                .as_ref()
                .filter(|change| change.tool != Some(tool))?
                .clone(),
        };
        change.tool = Some(tool);
        Some(change)
    }

    pub(crate) fn fix_button<T: 'static>(
        &self,
        handle: impl Fn(&mut T, FailureFix, &mut Window, &mut Context<T>) + 'static,
        cx: &mut Context<T>,
    ) -> Option<Button> {
        let fix = self.failure.as_ref()?.fix.clone()?;
        let label = match &fix {
            FailureFix::InstallDeveloperTools => "Install Developer Tools",
            FailureFix::OtherTool { tool, change } => {
                if change.is_none()
                    && !self
                        .change
                        .as_ref()
                        .is_some_and(|change| change.tool != Some(*tool))
                {
                    return None;
                }
                "Choose Another Source"
            }
            FailureFix::FreeUpSpace => "Free Up Space",
        };
        Some(
            Button::new("install-job-fix")
                .small()
                .label(label)
                .disabled(matches!(
                    self.developer_tools,
                    Some(DeveloperTools::Opening)
                ))
                .on_click(
                    cx.listener(move |this, _, window, cx| handle(this, fix.clone(), window, cx)),
                ),
        )
    }

    /// A project file pins the sources, so a repair can't swap them out.
    pub(crate) fn pinned_fix_button<T: 'static>(
        &self,
        handle: impl Fn(&mut T, FailureFix, &mut Window, &mut Context<T>) + 'static,
        cx: &mut Context<T>,
    ) -> Option<Button> {
        match self.failure.as_ref()?.fix.as_ref()? {
            FailureFix::OtherTool { .. } => None,
            _ => self.fix_button(handle, cx),
        }
    }

    pub(crate) fn request_developer_tools<T: 'static>(
        &mut self,
        daemon: &Daemon,
        job: impl for<'a> Fn(&'a mut T, &str) -> Option<&'a mut Job> + 'static,
        cx: &mut Context<T>,
    ) {
        if matches!(self.developer_tools, Some(DeveloperTools::Opening)) {
            return;
        }
        self.developer_tools = Some(DeveloperTools::Opening);
        let id = self.id.clone();
        let request = daemon.call::<()>(Call::InstallDeveloperTools);
        cx.spawn(async move |this, cx| {
            let result = request.await;
            this.update(cx, |this, cx| {
                if let Some(job) = job(this, &id) {
                    job.developer_tools = Some(if result.is_ok() {
                        DeveloperTools::Opened
                    } else {
                        DeveloperTools::Failed
                    });
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn render(&self, details: Button, fix: Option<Button>, p: &Palette) -> Div {
        let explanation = self.failure.as_ref();
        let status = match self.finished {
            Some(true) => format!("{} finished.", self.title),
            Some(false) => explanation
                .map(|failure| failure.message.clone())
                .unwrap_or_else(|| format!("{} didn't finish.", self.title)),
            None => match self
                .output
                .lines()
                .rev()
                .map(str::trim)
                .find(|l| !l.is_empty())
            {
                Some(last) => format!("{}: {last}", self.title),
                None => format!("{}…", self.title),
            },
        };
        let details = self.log.as_ref().map(|_| {
            details.label(if self.details {
                "Hide Details"
            } else {
                "Show Details"
            })
        });
        let status = line(
            status,
            if self.finished == Some(false) {
                p.err
            } else {
                p.muted
            },
        );
        let panel = if let Some(failure) = explanation {
            body()
                .child(status)
                .children(
                    failure
                        .detail
                        .iter()
                        .map(|detail| line(detail.clone(), p.muted)),
                )
                .children(fix.map(|button| h_flex().child(button)))
                .children(match self.developer_tools {
                    Some(DeveloperTools::Opening) => {
                        Some(line("Opening Apple's installer…", p.muted))
                    }
                    Some(DeveloperTools::Opened) => Some(line(
                        "Finish in Apple's installer, then try installing again.",
                        p.muted,
                    )),
                    Some(DeveloperTools::Failed) => {
                        Some(line("Couldn't open Apple's installer.", p.err))
                    }
                    None => None,
                })
                .children(details.map(|button| h_flex().child(button)))
        } else {
            body().child(
                h_flex()
                    .gap(px(look::GAP_8))
                    .child(status.flex_1())
                    .children(details),
            )
        };
        panel.when(self.details, |d| {
            d.child(
                div()
                    .id("install-job-log")
                    .max_h(px(220.))
                    .min_w_0()
                    .overflow_y_scroll()
                    .p(px(look::GAP_8))
                    .rounded(px(look::CONTROL_RADIUS))
                    .bg(p.ink(look::ROW_HOVER))
                    .font_family(look::mono())
                    .whitespace_normal()
                    .child(self.output.clone()),
            )
        })
    }
}

/// Share job tracking without tying it to either the Welcome or project window.
pub(crate) fn watch<T: 'static>(
    daemon: Rc<Daemon>,
    host: String,
    job: &Job,
    update: impl Fn(&mut T, &str, JobStatus, &mut Context<T>) -> bool + 'static,
    cx: &mut Context<T>,
) -> Task<()> {
    let id = job.id.clone();
    let log = job.log.clone();
    let initially_finished = job.finished;
    cx.spawn(async move |this, cx| {
        let mut seen = None;
        loop {
            if let Ok(terminals) = daemon.call::<Vec<TerminalInfo>>(Call::ListTerminals).await {
                let terminal = terminals.iter().find(|terminal| terminal.id == id);
                let finished = match terminal {
                    Some(t) if t.running => None,
                    Some(t) => Some(t.exit.as_ref().is_some_and(|e| e.code == Some(0))),
                    // Gone from the list once it ended and was cleaned up.
                    None => initially_finished.or(Some(true)),
                };
                let end = terminal.map(|t| t.end);
                let output = if let Some(log) = &log
                    && (end != seen || finished.is_some())
                {
                    daemon
                        .call::<FileText>(Call::ReadFile { path: log.clone() })
                        .await
                        .ok()
                        .map(|f| f.text)
                } else {
                    None
                };
                if output.is_some() {
                    seen = end;
                }
                let failure = match (&log, finished) {
                    (Some(log), Some(false)) => daemon
                        .call::<Option<InstallFailure>>(Call::ExplainInstallFailure {
                            log: log.clone(),
                        })
                        .await
                        .ok()
                        .flatten()
                        .filter(|failure| failure.kind != InstallFailureKind::Other),
                    _ => None,
                };
                let status = JobStatus {
                    output,
                    finished,
                    failure,
                };
                let keep = this
                    .update(cx, |this, cx| update(this, &id, status, cx))
                    .unwrap_or(false);
                if finished.is_some() {
                    cx.update(|cx| InstallCompletions::finished(host, cx));
                    break;
                }
                if !keep && log.is_none() {
                    break;
                }
            } else if this.update(cx, |_, _| ()).is_err() {
                break;
            }
            cx.background_executor().timer(Duration::from_secs(2)).await;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::{automatic_tool, copy_summary, progress_title, r_target};
    use sandhill_proto::{
        CopyRow, InstallChoice, InstallPrefs, InstallTarget, InstallTool, PackageAction,
        PackageChange,
    };

    #[test]
    fn copied_packages_are_grouped_by_source_and_version() {
        let row = |source: Option<&str>, version: Option<&str>, tool| CopyRow {
            name: "example".into(),
            language: "r".into(),
            version: "1.0".into(),
            from: "this computer".into(),
            source: source.map(str::to_owned),
            new_version: version.map(str::to_owned),
            tool,
            cant: source.is_none().then(|| "Not available.".into()),
        };
        assert_eq!(
            copy_summary(&[
                row(Some("conda-forge"), Some("1.0"), Some(InstallTool::Conda)),
                row(Some("bioconda"), Some("2.0"), Some(InstallTool::Conda)),
                row(Some("CRAN"), None, Some(InstallTool::R)),
                row(Some("Bioconductor"), None, Some(InstallTool::R)),
                row(Some("PyPI"), Some("1.0"), Some(InstallTool::Pip)),
                row(Some("GitHub"), Some("1.0"), Some(InstallTool::R)),
                row(None, None, None),
            ]),
            "1 from conda, same version; 1 from conda, another version; 2 from CRAN or Bioconductor, built here; 1 from PyPI; 1 from GitHub; 1 can't be copied"
        );
    }

    fn prefs(without_asking: bool, library: &str) -> InstallPrefs {
        InstallPrefs {
            without_asking,
            last: vec![InstallChoice {
                language: "r".into(),
                target: InstallTarget::R {
                    r: "/opt/R".into(),
                    library: library.into(),
                },
                tool: InstallTool::R,
            }],
            ..InstallPrefs::default()
        }
    }

    fn change(action: PackageAction, library: &str) -> PackageChange {
        PackageChange {
            action,
            target: InstallTarget::R {
                r: "/opt/R".into(),
                library: library.into(),
            },
            packages: vec!["DESeq2".into(), "edgeR".into()],
            tool: None,
            reason: String::new(),
        }
    }

    #[test]
    fn the_last_library_is_used_only_while_it_is_offered() {
        let libraries = vec!["/a".to_string(), "/b".to_string()];
        let target = |r: &str, prefs: &InstallPrefs| match r_target(r.into(), &libraries, prefs) {
            InstallTarget::R { library, .. } => library,
            InstallTarget::Python { .. } => unreachable!(),
        };
        assert_eq!(target("/opt/R", &prefs(false, "/b")), "/b");
        assert_eq!(target("/opt/R", &prefs(false, "/gone")), "/a");
        assert_eq!(target("/other/R", &prefs(false, "/b")), "/a");
    }

    #[test]
    fn only_installs_into_the_last_target_skip_the_question() {
        let on = prefs(true, "/b");
        assert_eq!(
            automatic_tool(&change(PackageAction::Install, "/b"), &on),
            Some(InstallTool::R)
        );
        assert_eq!(
            automatic_tool(&change(PackageAction::Install, "/a"), &on),
            None
        );
        assert_eq!(
            automatic_tool(&change(PackageAction::Update, "/b"), &on),
            None
        );
        assert_eq!(
            automatic_tool(&change(PackageAction::Remove, "/b"), &on),
            None
        );
        let off = prefs(false, "/b");
        assert_eq!(
            automatic_tool(&change(PackageAction::Install, "/b"), &off),
            None
        );
    }

    #[test]
    fn titles_name_the_packages() {
        assert_eq!(
            progress_title(&change(PackageAction::Install, "/b")),
            "Installing DESeq2 and edgeR"
        );
        let mut three = change(PackageAction::Remove, "/b");
        three.packages.push("limma".into());
        assert_eq!(progress_title(&three), "Removing DESeq2, edgeR and limma");
    }
}
