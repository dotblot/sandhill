//! Inline review and progress for changes to R, Python and their packages.

use std::time::Duration;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use sandhill_proto::{
    Call, EnvAction, EnvActionPlan, FileText, InstallPrefs, InstallTarget, InstallTool,
    InstallToolInfo, NewRuntime, PackageAction, PackageChange, PackagePlan, RuntimePlan,
    TerminalInfo,
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
    let summary = if plan.steps.is_empty() && plan.refused.is_none() {
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

/// The first-run review uses the same package details as every other install.
pub(crate) fn setup_packages<T: 'static>(
    reviews: &[Review<PackagePlan>],
    libraries: &[String],
    pick: impl Fn(&mut T, usize, Choice, &mut Context<T>) + Copy + 'static,
    p: &Palette,
    cx: &mut Context<T>,
) -> Div {
    body().children(reviews.iter().enumerate().map(|(ix, review)| match review {
        Review::Planning => line("Working out the command…", p.muted),
        Review::Ready(plan) => package_plan(ix, plan, libraries, pick, p, cx),
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

/// What runs, or why it can't, for one plan.
fn command_or_refusal(ix: usize, command: &str, refused: Option<&str>, p: &Palette) -> Div {
    v_flex()
        .min_w_0()
        .gap(px(look::GAP_6))
        .when_some(refused, |d, text| d.child(line(text.to_owned(), p.err)))
        .when(refused.is_none() && !command.is_empty(), |d| {
            d.child(command_with_copy(ix, command, p))
        })
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
    pick: impl Fn(&mut T, usize, Choice, &mut Context<T>) + Copy + 'static,
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
        .children(plan.packages.iter().map(|item| {
            let version = item.version.as_deref().unwrap_or("");
            let description = [item.name.as_str(), version]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            line(
                if item.source.is_empty() {
                    description
                } else {
                    format!("{description} ({})", item.source)
                },
                p.muted,
            )
        }));
    if plan.tools.len() > 1 {
        panel = panel.child(line("Tool", p.muted)).child(
            h_flex()
                .flex_wrap()
                .gap(px(look::GAP_4))
                .children(plan.tools.iter().map(|info| {
                    let tool = info.tool;
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
pub(crate) fn packages<T: 'static>(
    reviews: &[Review<PackagePlan>],
    skipped: &[String],
    libraries: &[String],
    (cancel, run): (Button, Button),
    pick: impl Fn(&mut T, usize, Choice, &mut Context<T>) + Copy + 'static,
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
    let mut panel = body();
    for (ix, plan) in plans.into_iter().enumerate() {
        panel = panel.child(package_plan(ix, plan, choices, pick, p, cx));
    }
    panel
        .children(skipped)
        .child(buttons(action, any, (cancel, run)))
}

pub(crate) fn runtime(
    review: &Review<RuntimePlan>,
    cancel: Button,
    run: Button,
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
    }
}

pub(crate) fn environment(
    review: &Review<EnvActionPlan>,
    name: &str,
    (cancel, run): (Button, Button),
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

pub(crate) fn cancel_button() -> Button {
    Button::new("install-review-cancel").small().label("Cancel")
}

pub(crate) fn run_button() -> Button {
    Button::new("install-review-run").small()
}

pub(crate) struct Job {
    pub(crate) id: String,
    pub(crate) log: Option<String>,
    pub(crate) title: String,
    pub(crate) output: String,
    pub(crate) finished: Option<bool>,
    pub(crate) details: bool,
}

impl Job {
    pub(crate) fn new(info: TerminalInfo, log: Option<String>, title: String) -> Self {
        Self {
            finished: (!info.running).then(|| info.exit.is_some_and(|e| e.code == Some(0))),
            id: info.id,
            log,
            title,
            output: String::new(),
            details: false,
        }
    }

    pub(crate) fn render(&self, details: Button, p: &Palette) -> Div {
        let status = match self.finished {
            Some(true) => format!("{} finished.", self.title),
            Some(false) => format!("{} didn't finish.", self.title),
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
        body()
            .child(
                h_flex()
                    .gap(px(look::GAP_8))
                    .child(
                        line(
                            status,
                            if self.finished == Some(false) {
                                p.err
                            } else {
                                p.muted
                            },
                        )
                        .flex_1(),
                    )
                    .when(self.log.is_some(), |d| {
                        d.child(details.label(if self.details {
                            "Hide Details"
                        } else {
                            "Show Details"
                        }))
                    }),
            )
            .when(self.details, |d| {
                // The log's end: long builds print thousands of lines.
                let tail: Vec<&str> = self.output.lines().rev().take(40).collect();
                let text = tail.into_iter().rev().collect::<Vec<_>>().join("\n");
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
                        .child(text),
                )
            })
    }
}

/// Share job tracking without tying it to either the Welcome or project window.
pub(crate) fn watch<T: 'static>(
    daemon: std::rc::Rc<Daemon>,
    job: &Job,
    update: impl Fn(&mut T, &str, Option<String>, Option<bool>, &mut Context<T>) -> bool + 'static,
    cx: &mut Context<T>,
) -> Task<()> {
    let id = job.id.clone();
    let log = job.log.clone();
    cx.spawn(async move |this, cx| {
        let mut seen = None;
        loop {
            if let Ok(terminals) = daemon.call::<Vec<TerminalInfo>>(Call::ListTerminals).await {
                let terminal = terminals.iter().find(|terminal| terminal.id == id);
                let finished = match terminal {
                    Some(t) if t.running => None,
                    Some(t) => Some(t.exit.as_ref().is_some_and(|e| e.code == Some(0))),
                    // Gone from the list once it ended and was cleaned up.
                    None => Some(true),
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
                let keep = this
                    .update(cx, |this, cx| update(this, &id, output, finished, cx))
                    .unwrap_or(false);
                if !keep || finished.is_some() {
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
    use super::{automatic_tool, progress_title, r_target};
    use sandhill_proto::{
        InstallChoice, InstallPrefs, InstallTarget, InstallTool, PackageAction, PackageChange,
    };

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
