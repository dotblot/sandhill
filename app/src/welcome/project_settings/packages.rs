//! Saving and setting up the package files a project carries.

use std::path::Path;
use std::rc::Rc;

use sandhill_proto::{
    FailureFix, ProjectFilePlan, ProjectPackageFile, ProjectSetupPlan, QuickInstalling,
};

use super::*;
use crate::install_confirm::{self as review, Job, Review};

pub(crate) const SET_UP_PACKAGES: &str = "Set Up This Project's Packages…";

#[derive(Default)]
pub(super) struct State {
    files: Option<Result<Vec<ProjectPackageFile>, String>>,
    files_request: Option<Task<()>>,
    action: Option<Action>,
}

struct Action {
    // A late result can't attach to a new action after the project changes.
    id: Rc<()>,
    document: Option<String>,
    stage: Stage,
}

enum Stage {
    Save(Review<ProjectFilePlan>, bool),
    Setup(Review<ProjectSetupPlan>),
    Writing,
    Starting,
    Job(Job),
    Message { text: String, failed: bool },
}

fn file_summary(file: &ProjectPackageFile) -> String {
    let count = file.packages;
    let packages = if count == 1 { "package" } else { "packages" };
    let title = format!("{}: {count} {packages}", file.file);
    if file.set_up {
        return format!("{title}, set up");
    }
    let count = file.differences.len();
    if count == 0 {
        return format!("{title}, not set up yet");
    }
    let examples = file.differences[..count.min(3)].join(", ");
    let more = if count > 3 {
        format!(" and {} more", count - 3)
    } else {
        String::new()
    };
    let differ = if count == 1 { "differs" } else { "differ" };
    format!("{title}. {count} {differ}: {examples}{more}")
}

impl Workbench {
    pub(crate) fn read_project_package_files(&mut self, again: bool, cx: &mut Context<Self>) {
        let state = &self.setup.packages;
        if !again && (state.files.is_some() || state.files_request.is_some()) {
            return;
        }
        let (Some(daemon), Some(project)) = (self.daemon(), self.project.clone()) else {
            return;
        };
        let generation = self.generation;
        let request = daemon.call::<Vec<ProjectPackageFile>>(Call::ProjectPackageFiles {
            project: project.clone(),
        });
        self.setup.packages.files_request = Some(cx.spawn(async move |this, cx| {
            let result = request.await.map_err(|error| error.to_string());
            let _ = this.update(cx, |this, cx| {
                if this.generation == generation && this.project.as_ref() == Some(&project) {
                    this.setup.packages.files = Some(result);
                    this.setup.packages.files_request = None;
                    cx.notify();
                }
            });
        }));
    }

    fn carries_document(&self, path: &str) -> bool {
        self.project
            .as_ref()
            .is_some_and(|project| Path::new(path).starts_with(project))
    }

    pub(crate) fn project_package_files_loading(&self, path: &str) -> bool {
        self.carries_document(path)
            && self.setup.packages.files.is_none()
            && self.setup.packages.files_request.is_some()
    }

    pub(super) fn project_package_file_needs_setup(&self, file: &str) -> bool {
        self.setup
            .packages
            .files
            .as_ref()
            .and_then(|files| files.as_ref().ok())
            .is_some_and(|files| {
                files
                    .iter()
                    .any(|entry| entry.file == file && !entry.set_up)
            })
    }

    pub(crate) fn project_package_file_for(&self, path: &str, language: &str) -> Option<&str> {
        if !self.carries_document(path) {
            return None;
        }
        self.setup
            .packages
            .files
            .as_ref()?
            .as_ref()
            .ok()?
            .iter()
            .find(|file| file.language == language && !file.set_up)
            .map(|file| file.file.as_str())
    }

    pub(crate) fn project_package_action_active(&self) -> bool {
        self.setup.packages.action.is_some()
    }

    pub(crate) fn showing_project_package_setup(&self, path: &str) -> bool {
        self.setup
            .packages
            .action
            .as_ref()
            .is_some_and(|action| action.document.as_deref() == Some(path))
    }

    pub(super) fn close_project_package_settings(&mut self) {
        if self.setup.packages.action.as_ref().is_some_and(|action| {
            action.document.is_none() && matches!(action.stage, Stage::Save(..) | Stage::Setup(_))
        }) {
            self.setup.packages.action = None;
        }
    }

    fn project_package_stage(&mut self, id: &Rc<()>) -> Option<&mut Stage> {
        self.setup
            .packages
            .action
            .as_mut()
            .filter(|action| Rc::ptr_eq(&action.id, id))
            .map(|action| &mut action.stage)
    }

    fn begin_project_package_action(
        &mut self,
        stage: Stage,
        document: Option<String>,
    ) -> Option<Rc<()>> {
        let state = &mut self.setup.packages;
        if state.action.is_some()
            || self
                .setup
                .dialog
                .as_ref()
                .is_some_and(|dialog| dialog.saving)
        {
            return None;
        }
        let id = Rc::new(());
        state.action = Some(Action {
            id: id.clone(),
            document,
            stage,
        });
        Some(id)
    }

    fn ask_project_package_save(&mut self, language: &'static str, cx: &mut Context<Self>) {
        let (Some(daemon), Some(project)) = (self.daemon(), self.project.clone()) else {
            return;
        };
        let Some(id) =
            self.begin_project_package_action(Stage::Save(Review::Planning, false), None)
        else {
            return;
        };
        let request = daemon.call::<ProjectFilePlan>(Call::PlanProjectFile {
            project,
            language: language.into(),
        });
        cx.spawn(async move |this, cx| {
            let result = request.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(stage) = this.project_package_stage(&id) {
                    *stage = Stage::Save(result.into(), false);
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn ask_project_package_setup(
        &mut self,
        file: String,
        document: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let (Some(daemon), Some(project)) = (self.daemon(), self.project.clone()) else {
            return;
        };
        let Some(id) = self.begin_project_package_action(Stage::Setup(Review::Planning), document)
        else {
            return;
        };
        let request = daemon.call::<ProjectSetupPlan>(Call::PlanProjectSetup { project, file });
        cx.spawn(async move |this, cx| {
            let result = request.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(stage) = this.project_package_stage(&id) {
                    *stage = Stage::Setup(result.into());
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn write_project_package_file(&mut self, id: Rc<()>, cx: &mut Context<Self>) {
        let (Some(daemon), Some(project)) = (self.daemon(), self.project.clone()) else {
            return;
        };
        let Some(stage) = self.project_package_stage(&id) else {
            return;
        };
        let Stage::Save(Review::Ready(plan), _) = stage else {
            return;
        };
        if plan.refused.is_some() {
            return;
        }
        let language = plan.language.clone();
        *stage = Stage::Writing;
        let request = daemon.call::<ProjectFilePlan>(Call::WriteProjectFile { project, language });
        cx.spawn(async move |this, cx| {
            let result = request.await;
            let _ = this.update(cx, |this, cx| {
                let Some(stage) = this.project_package_stage(&id) else {
                    return;
                };
                match result {
                    Ok(plan) if plan.refused.is_none() => {
                        *stage = Stage::Message {
                            text: format!("Saved {}.", plan.path),
                            failed: false,
                        };
                        this.read_project_package_files(true, cx);
                        this.refresh_project_package_context(false, cx);
                        this.read_folder(String::new(), cx);
                    }
                    Ok(plan) => *stage = Stage::Save(Review::Ready(plan), false),
                    Err(error) => {
                        *stage = Stage::Message {
                            text: format!("Couldn't save the package file: {error:#}"),
                            failed: true,
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn run_project_package_setup(&mut self, id: Rc<()>, cx: &mut Context<Self>) {
        if self.held_back("set up this project's packages", cx) {
            return;
        }
        let (Some(daemon), Some(project)) = (self.daemon(), self.project.clone()) else {
            return;
        };
        let Some(stage) = self.project_package_stage(&id) else {
            return;
        };
        let Stage::Setup(Review::Ready(plan)) = stage else {
            return;
        };
        if plan.refused.is_some() || plan.steps.is_empty() {
            return;
        }
        let file = plan.file.clone();
        *stage = Stage::Starting;
        let host = self.host();
        let request = daemon.call::<QuickInstalling>(Call::RunProjectSetup { project, file });
        cx.spawn(async move |this, cx| {
            let result = request.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(started) => {
                        let job = Job::new(
                            started.job,
                            Some(started.log),
                            "Setting up this project's packages".into(),
                        );
                        let watch_id = id.clone();
                        review::watch(
                            daemon,
                            host,
                            &job,
                            move |this: &mut Self, job_id, status, cx| {
                                let Some(job) = this.project_package_job(&watch_id, job_id) else {
                                    return false;
                                };
                                job.update(status);
                                if job.finished == Some(true) {
                                    this.refresh_project_package_context(true, cx);
                                    this.read_folder(String::new(), cx);
                                }
                                cx.notify();
                                true
                            },
                            cx,
                        )
                        .detach();
                        if let Some(stage) = this.project_package_stage(&id) {
                            *stage = Stage::Job(job);
                        }
                    }
                    Err(error) => {
                        if let Some(stage) = this.project_package_stage(&id) {
                            *stage = Stage::Message {
                                text: format!("Couldn't start the setup: {error:#}"),
                                failed: true,
                            };
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn refresh_project_package_context(&mut self, reload_kernels: bool, cx: &mut Context<Self>) {
        let (Some(daemon), Some(path)) = (self.daemon(), self.project.clone()) else {
            return;
        };
        let generation = self.generation;
        let settings = daemon.call::<ProjectSetup>(Call::GetProjectSettings { path: path.clone() });
        let kernels = reload_kernels.then(|| {
            daemon.call::<Vec<KernelspecInfo>>(Call::ListProjectKernelspecs { path: path.clone() })
        });
        cx.spawn(async move |this, cx| {
            let settings = settings.await;
            let kernels = match kernels {
                Some(request) => request.await.ok(),
                None => None,
            };
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation || this.project.as_ref() != Some(&path) {
                    return;
                }
                if let Ok(settings) = settings {
                    if let Some(dialog) = &mut this.setup.dialog {
                        dialog.setup = settings.clone();
                    }
                    this.setup.setup = Some(settings);
                }
                if let Some(kernels) = kernels {
                    if let Some(dialog) = &mut this.setup.dialog {
                        dialog.kernels.clone_from(&kernels);
                    }
                    this.setup.kernels.clone_from(&kernels);
                    this.kernels = kernels;
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn project_package_job(&mut self, id: &Rc<()>, job_id: &str) -> Option<&mut Job> {
        match self.project_package_stage(id)? {
            Stage::Job(job) if job.id == job_id => Some(job),
            _ => None,
        }
    }

    pub(super) fn render_project_package_settings(
        &self,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let state = &self.setup.packages;
        let busy = state.action.is_some()
            || self
                .setup
                .dialog
                .as_ref()
                .is_some_and(|dialog| dialog.saving);
        let mut rows = Vec::new();
        if self.restricted() {
            rows.push(
                look::settings_row(
                    "Restricted Mode",
                    Some(look::settings_detail(
                        "Trust this project before setting up its packages.",
                        p,
                    )),
                )
                .into_any_element(),
            );
        }
        for (language, title) in [("r", "R"), ("python", "Python")] {
            rows.push(
                look::settings_row(title, None)
                    .child(
                        Button::new(("project-packages-save", usize::from(language == "python")))
                            .small()
                            .label("Save This Project's Packages…")
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.ask_project_package_save(language, cx);
                            })),
                    )
                    .into_any_element(),
            );
            if let Some(Ok(files)) = &state.files {
                rows.extend(
                    files
                        .iter()
                        .filter(|file| file.language == language)
                        .map(|file| {
                            let name = file.file.clone();
                            look::settings_row(file_summary(file), None)
                                .child(
                                    Button::new(SharedString::from(format!(
                                        "project-packages-setup-{name}"
                                    )))
                                    .small()
                                    .label(SET_UP_PACKAGES)
                                    .disabled(busy)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.ask_project_package_setup(name.clone(), None, cx);
                                    })),
                                )
                                .into_any_element()
                        }),
                );
            }
        }
        match &state.files {
            None => rows
                .push(look::settings_row("Checking for package files…", None).into_any_element()),
            Some(Err(error)) => rows.push(
                look::settings_row(
                    "Couldn't read the package files",
                    Some(look::settings_detail(error.clone(), p)),
                )
                .into_any_element(),
            ),
            Some(Ok(_)) => {}
        }
        if let Some(action) = &state.action {
            rows.push(
                self.render_project_package_action(action, p, cx)
                    .into_any_element(),
            );
        }
        look::settings_group("This project's packages", rows, p)
    }

    pub(crate) fn render_document_project_setup(
        &self,
        path: &str,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        let action = self
            .setup
            .packages
            .action
            .as_ref()
            .filter(|action| action.document.as_deref() == Some(path))?;
        Some(self.render_project_package_action(action, p, cx))
    }

    fn render_project_package_action(
        &self,
        action: &Action,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        let id = action.id.clone();
        let close = |label, cx: &mut Context<Self>| {
            let id = action.id.clone();
            review::cancel_button()
                .label(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    if this.project_package_stage(&id).is_some() {
                        this.setup.packages.action = None;
                        cx.notify();
                    }
                }))
        };
        let done = |cx: &mut Context<Self>| h_flex().justify_end().child(close("Done", cx));
        let status = |message: SharedString, failed| {
            v_flex()
                .px(px(look::PANE_PAD_X))
                .py(px(look::PANE_PAD_Y))
                .gap(px(look::GAP_6))
                .child(look::settings_detail(message, p).when(failed, |d| d.text_color(p.err)))
        };
        match &action.stage {
            Stage::Save(plan, expanded) => {
                let contents_id = id.clone();
                let contents = Button::new("project-package-file-contents")
                    .small()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(Stage::Save(_, expanded)) =
                            this.project_package_stage(&contents_id)
                        {
                            *expanded = !*expanded;
                            cx.notify();
                        }
                    }));
                let save = review::run_button().on_click(cx.listener(move |this, _, _, cx| {
                    this.write_project_package_file(id.clone(), cx);
                }));
                review::project_file(plan, *expanded, contents, (close("Cancel", cx), save), p)
            }
            Stage::Setup(plan) => {
                let run = review::run_button().on_click(cx.listener(move |this, _, _, cx| {
                    this.run_project_package_setup(id.clone(), cx);
                }));
                review::project_setup(plan, (close("Cancel", cx), run), p)
            }
            Stage::Writing => status("Saving this project's packages…".into(), false),
            Stage::Starting => status("Starting the setup…".into(), false),
            Stage::Message { text, failed } => status(text.clone().into(), *failed).child(done(cx)),
            Stage::Job(job) => {
                let job_id = job.id.clone();
                let details_id = id.clone();
                let details =
                    Button::new("project-packages-details")
                        .small()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(job) = this.project_package_job(&details_id, &job_id) {
                                job.details = !job.details;
                                cx.notify();
                            }
                        }));
                let job_id = job.id.clone();
                let fix = job.pinned_fix_button(
                    move |this: &mut Self, fix, _, cx| {
                        if fix == FailureFix::FreeUpSpace {
                            this.manage_resources(cx);
                        } else if let Some(daemon) = this.daemon()
                            && let Some(job) = this.project_package_job(&id, &job_id)
                        {
                            let id = id.clone();
                            job.request_developer_tools(
                                &daemon,
                                move |this: &mut Self, job_id| {
                                    this.project_package_job(&id, job_id)
                                },
                                cx,
                            );
                        }
                    },
                    cx,
                );
                v_flex()
                    .child(job.render(details, fix, p))
                    .when(job.finished.is_some(), |d| {
                        d.child(done(cx).px(px(look::PANE_PAD_X)).pb(px(look::PANE_PAD_Y)))
                    })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ProjectPackageFile, file_summary};

    #[test]
    fn package_file_summary_distinguishes_setup_from_differences() {
        let mut file = ProjectPackageFile {
            file: "renv.lock".into(),
            language: "r".into(),
            packages: 42,
            set_up: true,
            differences: Vec::new(),
        };
        assert_eq!(file_summary(&file), "renv.lock: 42 packages, set up");
        file.set_up = false;
        assert_eq!(
            file_summary(&file),
            "renv.lock: 42 packages, not set up yet"
        );
        file.differences.push("Seurat 4.4.0 (have 5.1.0)".into());
        assert_eq!(
            file_summary(&file),
            "renv.lock: 42 packages. 1 differs: Seurat 4.4.0 (have 5.1.0)"
        );
    }
}
