//! Missing packages at the top of a document and under the run that needs them.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::{Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use sandhill_proto::{
    FailureFix, InstallPrefs, MissingPackages, PackageAction, PackageChange, PackagePlan,
    QuickInstalling,
};

use super::*;
use crate::install_confirm::{self as review, Choice, InstallCompletions, Job, Review};
use crate::look::{self, Palette};

#[derive(Default)]
pub(super) struct MissingBar {
    dismissed: bool,
    scan: u64,
    kernel: Option<String>,
    session: Option<String>,
    found: Option<Offer>,
    install: Option<Box<Attempt>>,
}

impl MissingBar {
    pub(crate) fn visible(&self) -> bool {
        !self.dismissed && (self.found.is_some() || self.install.is_some())
    }
}

impl Open {
    pub(crate) fn has_missing_package_bar(&self) -> bool {
        self.missing.visible()
    }
}

struct Offer {
    change: PackageChange,
    prompt: SharedString,
}

impl Offer {
    fn new(found: MissingPackages, format: Format) -> Option<Self> {
        if found.missing.is_empty() {
            return None;
        }
        let mut change = change_for(found, String::new());
        let kind = document_kind(format);
        let names = review::names(&change.packages);
        let (reason, question) = if change.packages.len() == 1 {
            ("it", "isn't installed. Install it?")
        } else {
            ("them", "aren't installed. Install them?")
        };
        change.reason = format!("This {kind} uses {reason}");
        Some(Self {
            change,
            prompt: format!("This {kind} uses {names}, which {question}").into(),
        })
    }
}

#[derive(Clone)]
enum Place {
    Document(String),
    Error(String),
}

pub(super) struct Attempt {
    id: u64,
    stage: Stage,
}

enum Stage {
    Checking,
    Review(Review<PackagePlan>),
    Starting,
    Job(Job, Option<String>),
    Installed,
    Message(&'static str),
}

impl Run {
    /// Only error outputs qualify. The daemon parses the name and checks whether
    /// it is actually missing before we make any plan.
    fn missing_error(&self) -> Option<(&str, &str)> {
        if !self.finished || self.session.is_empty() {
            return None;
        }
        self.outputs.iter().rev().find_map(|out| {
            if out["output_type"] != "error" {
                return None;
            }
            let name = out["ename"].as_str().unwrap_or_default();
            let value = out["evalue"].as_str().unwrap_or_default();
            if (name == "ModuleNotFoundError" && !value.is_empty())
                || missing_message(name)
                || missing_message(value)
            {
                return Some((name, value));
            }
            out["traceback"]
                .as_array()?
                .iter()
                .rev()
                .filter_map(Value::as_str)
                .find(|line| missing_message(line))
                .map(|line| (name, line))
        })
    }
}

fn missing_message(text: &str) -> bool {
    text.contains("there is no package called")
        || text.contains("ModuleNotFoundError")
        || text.contains("No module named")
        || (text.contains("package ") && text.contains(" required by"))
}

fn document_kind(format: Format) -> &'static str {
    match format {
        Format::Script => "script",
        Format::Quarto => "Quarto file",
        _ => "notebook",
    }
}

fn change_for(found: MissingPackages, reason: String) -> PackageChange {
    let mut packages = Vec::new();
    for item in found.missing {
        if !packages.contains(&item.package) {
            packages.push(item.package);
        }
    }
    PackageChange {
        action: PackageAction::Install,
        target: found.target,
        packages,
        tool: None,
        reason,
    }
}

fn needs_scan(open: &Open, force: bool, restricted: bool) -> bool {
    let bar = &open.missing;
    matches!(
        open.notebook.format,
        Format::Ipynb | Format::Quarto | Format::Script
    ) && !bar.dismissed
        && (force
            || bar.scan == 0
            || bar.kernel != open.kernel
            || bar.session.as_deref() != scan_session(open, restricted))
}

fn scan_session(open: &Open, restricted: bool) -> Option<&str> {
    if restricted { None } else { open.session.id() }
}

fn kernel_busy(error: &anyhow::Error) -> bool {
    error
        .to_string()
        .contains("the session is busy running code")
}

impl Workbench {
    pub(super) fn packages_installed(&mut self, cx: &mut Context<Self>) {
        let host = self.host();
        let count = InstallCompletions::count(&host, cx);
        if count != 0 && (self.missing_completion.0 != host || self.missing_completion.1 != count) {
            self.missing_completion = (host, count);
            self.refresh_packages(cx);
            self.refresh_missing_documents(true, cx);
        }
    }

    pub(super) fn refresh_missing_documents(&mut self, force: bool, cx: &mut Context<Self>) {
        let restricted = self.restricted();
        let paths: Vec<_> = self
            .open
            .iter()
            .chain(&self.others)
            .filter(|open| needs_scan(open, force, restricted))
            .map(|open| open.notebook.path.clone())
            .collect();
        for path in paths {
            self.refresh_document_packages(&path, force, cx);
        }
    }

    pub(super) fn refresh_document_packages(
        &mut self,
        path: &str,
        force: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(daemon) = self.daemon() else {
            return;
        };
        let project = self.project.clone();
        let restricted = self.restricted();
        self.next_missing_request += 1;
        let id = self.next_missing_request;
        let Some(open) = self
            .open_at(path)
            .filter(|open| needs_scan(open, force, restricted))
        else {
            return;
        };
        let session = scan_session(open, restricted).map(str::to_owned);
        let format = open.notebook.format;
        let bar = &mut open.missing;
        if bar.kernel != open.kernel {
            bar.install = None;
            bar.found = None;
        }
        bar.scan = id;
        bar.kernel.clone_from(&open.kernel);
        bar.session = session;
        let path = path.to_owned();
        let check = daemon.call::<MissingPackages>(Call::MissingPackages {
            path: path.clone(),
            kernel: bar.kernel.clone(),
            session: bar.session.clone(),
            project,
        });
        cx.spawn(async move |this, cx| {
            let found = check.await;
            this.update(cx, |this, cx| {
                if let Some(bar) = this.open_at(&path).map(|open| &mut open.missing)
                    && bar.scan == id
                    && !bar.dismissed
                {
                    if found.as_ref().is_err_and(kernel_busy) {
                        // Wait for the run's finish event instead of losing a
                        // still-valid offer for this kernel.
                        bar.scan = 0;
                        return;
                    }
                    bar.found = found.ok().and_then(|found| Offer::new(found, format));
                    if matches!(
                        bar.install.as_deref().map(|attempt| &attempt.stage),
                        Some(Stage::Installed)
                    ) {
                        bar.install = None;
                    }
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn install_slot(&mut self, place: &Place) -> Option<&mut Option<Box<Attempt>>> {
        match place {
            Place::Document(path) => self.open_at(path).map(|open| &mut open.missing.install),
            Place::Error(run) => self.runs.get_mut(run).map(|run| &mut run.missing_install),
        }
    }

    fn install_attempt(&mut self, place: &Place, id: u64) -> Option<&mut Attempt> {
        self.install_slot(place)?
            .as_deref_mut()
            .filter(|a| a.id == id)
    }

    fn start_attempt(&mut self, place: &Place, cx: &mut Context<Self>) -> Option<u64> {
        self.next_missing_request += 1;
        let id = self.next_missing_request;
        let slot = self.install_slot(place)?;
        if slot.is_some() {
            return None;
        }
        *slot = Some(Box::new(Attempt {
            id,
            stage: Stage::Checking,
        }));
        cx.notify();
        Some(id)
    }

    fn ask_document_install(&mut self, path: String, cx: &mut Context<Self>) {
        if self.held_back("install packages", cx) {
            return;
        }
        let Some(open) = self.open_at(&path).filter(|open| !open.missing.dismissed) else {
            return;
        };
        let Some(change) = open
            .missing
            .found
            .as_ref()
            .map(|offer| offer.change.clone())
        else {
            return;
        };
        let place = Place::Document(path);
        if let Some(id) = self.start_attempt(&place, cx) {
            self.plan_missing_install(place, id, change, true, cx);
        }
    }

    fn ask_error_install(&mut self, run_id: String, cx: &mut Context<Self>) {
        if self.held_back("install packages", cx) {
            return;
        }
        let Some(daemon) = self.daemon() else {
            return;
        };
        let Some(run) = self.runs.get(&run_id) else {
            return;
        };
        let Some((name, message)) = run.missing_error() else {
            return;
        };
        let session = run.session.clone();
        let error = format!("{name}: {message}");
        let place = Place::Error(run_id);
        let Some(id) = self.start_attempt(&place, cx) else {
            return;
        };
        let check = daemon.call::<MissingPackages>(Call::MissingInError { session, error });
        cx.spawn(async move |this, cx| {
            let checked = check.await;
            this.update(cx, |this, cx| {
                let Some(attempt) = this.install_attempt(&place, id) else {
                    return;
                };
                let message = match checked {
                    Ok(found) if !found.missing.is_empty() => {
                        this.plan_missing_install(
                            place,
                            id,
                            change_for(found, "Needed to run this code".into()),
                            true,
                            cx,
                        );
                        return;
                    }
                    Ok(_) => "Sandhill couldn't find a missing package in this error.",
                    Err(error) if kernel_busy(&error) => {
                        "Wait for the current run to finish, then try installing again."
                    }
                    Err(_) => "Sandhill couldn't check which package this run needs.",
                };
                attempt.stage = Stage::Message(message);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn plan_missing_install(
        &mut self,
        place: Place,
        id: u64,
        mut change: PackageChange,
        with_saved_choice: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(daemon) = self.daemon() else {
            return;
        };
        let Some(attempt) = self.install_attempt(&place, id) else {
            return;
        };
        attempt.stage = Stage::Review(Review::Planning);
        cx.spawn(async move |this, cx| {
            let automatic = if with_saved_choice {
                let prefs = daemon
                    .call::<InstallPrefs>(Call::InstallPrefs)
                    .await
                    .unwrap_or_default();
                review::automatic_tool(&change, &prefs)
            } else {
                None
            };
            if automatic.is_some() {
                change.tool = automatic;
            }
            let target = automatic.map(|_| change.target.clone());
            let planned = daemon
                .call::<PackagePlan>(Call::PlanPackages { change })
                .await;
            this.update(cx, |this, cx| {
                let Some(attempt) = this.install_attempt(&place, id) else {
                    return;
                };
                if let (Some(tool), Ok(plan)) = (automatic, &planned)
                    && plan.change.tool == Some(tool)
                    && target.as_ref() == Some(&plan.change.target)
                    && review::runnable(plan)
                {
                    this.run_missing_install(place, id, plan, true, cx);
                } else {
                    attempt.stage = Stage::Review(planned.into());
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn run_missing_install(
        &mut self,
        place: Place,
        id: u64,
        plan: &PackagePlan,
        automatic: bool,
        cx: &mut Context<Self>,
    ) {
        if self.held_back("install packages", cx) {
            return;
        }
        let Some(daemon) = self.daemon() else {
            return;
        };
        let host = self.host();
        let Some(attempt) = self.install_attempt(&place, id) else {
            return;
        };
        attempt.stage = Stage::Starting;
        let change = plan.change.clone();
        let title = review::progress_title(&change);
        let notice = automatic.then(|| review::auto_notice(plan));
        let started = daemon.call::<QuickInstalling>(Call::RunPackages {
            change: change.clone(),
        });
        cx.spawn(async move |this, cx| {
            let started = started.await;
            this.update(cx, |this, cx| {
                match started {
                    Ok(started) => {
                        let mut job = Job::new(started.job, Some(started.log), title);
                        job.change = Some(change);
                        let watched = place.clone();
                        review::watch(
                            daemon,
                            host,
                            &job,
                            move |this: &mut Self, job_id, status, cx| {
                                let Some(attempt) = this.install_attempt(&watched, id) else {
                                    return false;
                                };
                                let Stage::Job(job, notice) = &mut attempt.stage else {
                                    return false;
                                };
                                if job.id != job_id {
                                    return false;
                                }
                                job.update(status);
                                if job.finished == Some(true) {
                                    let notice = notice.take();
                                    attempt.stage = Stage::Installed;
                                    if let Some(notice) = notice {
                                        this.notice = Some(notice);
                                    }
                                }
                                cx.notify();
                                true
                            },
                            cx,
                        )
                        .detach();
                        if let Some(attempt) = this.install_attempt(&place, id) {
                            attempt.stage = Stage::Job(job, notice);
                        }
                    }
                    Err(_) => {
                        if let Some(attempt) = this.install_attempt(&place, id) {
                            attempt.stage = Stage::Message("Couldn't start the install.");
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

    fn inline_job(&mut self, place: &Place, id: u64, job_id: &str) -> Option<&mut Job> {
        match &mut self.install_attempt(place, id)?.stage {
            Stage::Job(job, _) if job.id == job_id => Some(job),
            _ => None,
        }
    }

    fn inline_job_fix(
        &mut self,
        place: Place,
        id: u64,
        job_id: &str,
        fix: FailureFix,
        cx: &mut Context<Self>,
    ) {
        let Some(daemon) = self.daemon() else {
            return;
        };
        let Some(job) = self.inline_job(&place, id, job_id) else {
            return;
        };
        match fix {
            FailureFix::InstallDeveloperTools => job.request_developer_tools(
                &daemon,
                move |this: &mut Self, job_id| this.inline_job(&place, id, job_id),
                cx,
            ),
            FailureFix::OtherTool { tool, change } => {
                if let Some(change) = job.alternative(tool, change) {
                    self.plan_missing_install(place, id, change, false, cx);
                }
            }
            FailureFix::FreeUpSpace => self.manage_resources(cx),
        }
    }

    fn render_inline_install(
        &self,
        place: Place,
        attempt: &Attempt,
        cell: bool,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let id = attempt.id;
        let cancel_at = place.clone();
        let cancel = cx.listener(move |this, _, _, cx| {
            if let Some(slot) = this.install_slot(&cancel_at)
                && slot.as_ref().is_some_and(|attempt| attempt.id == id)
            {
                *slot = None;
                cx.notify();
            }
        });
        match &attempt.stage {
            Stage::Checking => inline_line("Checking which package is missing…", p.muted),
            Stage::Starting => inline_line("Starting the install…", p.muted),
            Stage::Message(message) => v_flex().child(inline_line(message, p.err)).child(
                h_flex().child(
                    look::button("missing-done", "Done", None, look::ButtonSize::Row)
                        .on_click(cancel),
                ),
            ),
            Stage::Review(planned) => {
                let run_at = place.clone();
                let run = review::run_button().on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(Attempt {
                        stage: Stage::Review(Review::Ready(plan)),
                        ..
                    }) = this.install_attempt(&run_at, id)
                        && review::runnable(plan)
                    {
                        let plan = plan.clone();
                        this.run_missing_install(run_at.clone(), id, &plan, false, cx);
                    }
                }));
                review::packages(
                    std::slice::from_ref(planned),
                    &[],
                    &[],
                    (review::cancel_button().on_click(cancel), run),
                    move |this: &mut Self, _, choice: Choice, cx| {
                        if let Some(Attempt {
                            stage: Stage::Review(Review::Ready(plan)),
                            ..
                        }) = this.install_attempt(&place, id)
                        {
                            let mut change = plan.change.clone();
                            choice.apply(&mut change);
                            if change != plan.change {
                                this.plan_missing_install(place.clone(), id, change, false, cx);
                            }
                        }
                    },
                    p,
                    cx,
                )
            }
            Stage::Installed => inline_line(
                match (place, cell) {
                    (Place::Document(_), _) => "Installed.",
                    (_, true) => "Installed. Run the cell again.",
                    (_, false) => "Installed. Run it again.",
                },
                p.text,
            ),
            Stage::Job(job, _) => {
                let details_at = place.clone();
                let job_id = job.id.clone();
                let details = Button::new("missing-job-details")
                    .small()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(job) = this.inline_job(&details_at, id, &job_id) {
                            job.details = !job.details;
                            cx.notify();
                        }
                    }));
                let job_id = job.id.clone();
                let fix = job.fix_button(
                    move |this, fix, _, cx| {
                        this.inline_job_fix(place.clone(), id, &job_id, fix, cx);
                    },
                    cx,
                );
                job.render(details, fix, p)
                    .when(job.finished == Some(false), |d| {
                        d.child(
                            h_flex().justify_end().child(
                                look::button("missing-done", "Done", None, look::ButtonSize::Row)
                                    .on_click(cancel),
                            ),
                        )
                    })
            }
        }
    }

    pub(crate) fn render_missing_bar(
        &self,
        open: &Open,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !open.missing.visible() {
            return None;
        }
        let path = &open.notebook.path;
        let content = if let Some(attempt) = open.missing.install.as_deref() {
            self.render_inline_install(Place::Document(path.clone()), attempt, false, p, cx)
        } else {
            let prompt = open.missing.found.as_ref()?.prompt.clone();
            let install_path = path.clone();
            let dismiss_path = path.clone();
            h_flex()
                .flex_wrap()
                .gap(px(look::GAP_6))
                .px(px(look::PANE_PAD_X))
                .py(px(look::PANE_PAD_Y))
                .child(
                    div()
                        .min_w_0()
                        .whitespace_normal()
                        .text_size(px(look::small()))
                        .text_color(p.text)
                        .child(prompt),
                )
                .child(
                    h_flex()
                        .flex_none()
                        .gap(px(look::GAP_6))
                        .child(
                            look::button("missing-not-now", "Not Now", None, look::ButtonSize::Row)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(open) = this.open_at(&dismiss_path) {
                                        open.missing.dismissed = true;
                                        open.missing.found = None;
                                        cx.notify();
                                    }
                                })),
                        )
                        .child(
                            look::button(
                                "missing-install",
                                "Install…",
                                None,
                                look::ButtonSize::Row,
                            )
                            .primary()
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.ask_document_install(install_path.clone(), cx);
                                },
                            )),
                        ),
                )
        };
        Some(
            div()
                .id("document-missing-packages")
                .flex_none()
                .max_h(relative(0.6))
                .overflow_y_scroll()
                .border_b_1()
                .border_color(p.glass_edge)
                .child(content)
                .into_any_element(),
        )
    }

    pub(crate) fn render_error_install(
        &self,
        run_id: &str,
        run: &Run,
        cell: bool,
        cx: &mut Context<Self>,
    ) -> (Option<Button>, Option<Div>) {
        if let Some(attempt) = run.missing_install.as_deref() {
            let p = *Palette::get(cx);
            return (
                None,
                Some(self.render_inline_install(
                    Place::Error(run_id.to_owned()),
                    attempt,
                    cell,
                    &p,
                    cx,
                )),
            );
        }
        if run.missing_error().is_none() || self.restricted() {
            return (None, None);
        }
        let id = run_id.to_owned();
        let button = look::button(
            "install-error-package",
            "Install",
            None,
            look::ButtonSize::Row,
        )
        .on_click(cx.listener(move |this, _, _, cx| this.ask_error_install(id.clone(), cx)));
        (Some(button), None)
    }
}

fn inline_line(text: &'static str, color: Hsla) -> Div {
    div()
        .px(px(look::PANE_PAD_X))
        .py(px(look::PANE_PAD_Y))
        .min_w_0()
        .whitespace_normal()
        .text_size(px(look::small()))
        .text_color(color)
        .child(text)
}

#[cfg(test)]
mod tests {
    use super::{Format, Offer, Run};
    use sandhill_proto::{InstallTarget, MissingPackage, MissingPackages};
    use serde_json::json;

    #[test]
    fn document_prompt_names_installable_packages_in_order() {
        let offer = |names: &[(&str, &str)], format| {
            Offer::new(
                MissingPackages {
                    language: "python".into(),
                    target: InstallTarget::Python {
                        prefix: "/example".into(),
                        python: "/example/bin/python".into(),
                    },
                    kernel: "python".into(),
                    missing: names
                        .iter()
                        .map(|(import, package)| MissingPackage {
                            import: (*import).into(),
                            package: (*package).into(),
                        })
                        .collect(),
                },
                format,
            )
        };
        let one = offer(&[("sklearn", "scikit-learn")], Format::Script).unwrap();
        assert_eq!(
            one.prompt.as_ref(),
            "This script uses scikit-learn, which isn't installed. Install it?"
        );
        assert_eq!(one.change.reason, "This script uses it");

        let many = offer(
            &[
                ("sklearn", "scikit-learn"),
                ("other", "scikit-learn"),
                ("PIL", "pillow"),
            ],
            Format::Quarto,
        )
        .unwrap();
        assert_eq!(many.change.packages, ["scikit-learn", "pillow"]);
        assert_eq!(
            many.prompt.as_ref(),
            "This Quarto file uses scikit-learn and pillow, which aren't installed. Install them?"
        );
        assert!(offer(&[], Format::Ipynb).is_none());
    }

    #[test]
    fn only_finished_errors_with_a_session_offer_install() {
        let python = json!({
            "output_type": "error", "ename": "ModuleNotFoundError",
            "evalue": "No module named 'sklearn.metrics'",
        });
        let r = json!({
            "output_type": "error", "ename": "Error", "evalue": "",
            "traceback": ["Error in library(Seurat): there is no package called ‘Seurat’"],
        });
        let mut run = Run {
            outputs: vec![python],
            finished: true,
            session: "kernel".into(),
            ..Default::default()
        };
        assert_eq!(
            run.missing_error(),
            Some(("ModuleNotFoundError", "No module named 'sklearn.metrics'"))
        );
        run.outputs = vec![r];
        assert!(run.missing_error().unwrap().1.contains("Seurat"));
        run.outputs = vec![json!({
            "output_type": "stream", "name": "stderr",
            "text": "ModuleNotFoundError: No module named 'sklearn'",
        })];
        assert!(run.missing_error().is_none());
        run.outputs = vec![json!({"output_type": "error", "ename": "KeyError", "evalue": "x"})];
        assert!(run.missing_error().is_none());
        run.outputs[0]["ename"] = json!("ModuleNotFoundError");
        run.finished = false;
        assert!(run.missing_error().is_none());
        run.finished = true;
        run.session.clear();
        assert!(run.missing_error().is_none());
    }
}
