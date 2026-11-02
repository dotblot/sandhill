//! Project settings, as the Tauri app has them (`ProjectSettings.tsx`): the
//! R and Python each language runs in, where notebooks and Quarto
//! documents run, the env files kernels start with, and where agent
//! conversations are kept. The daemon saves them in
//! `.sandhill/project.toml`, so collaborators get them too. The first time
//! a project opens without saved settings, this asks once; the gear in the
//! title bar brings it back.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Disableable, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use sandhill_proto::{
    Call, ChatHistory, DocumentsRunIn, FoundEnvironment, KernelspecInfo, ProjectSettings,
    ProjectSetup,
};

use crate::look::{self, Palette, icon};
use crate::workbench::Workbench;

/// Why a language's list has no other kernel to pick.
fn only_kernel(language: &str) -> &'static str {
    if language == "r" {
        "Only R installations with a kernel (Ark, or IRkernel) are listed."
    } else {
        "Only environments with ipykernel are listed (for a conda env: `conda install -n <env> ipykernel`)."
    }
}

/// "Found renv.lock (renv)." or "Found environment.yml (conda env
/// omicverse).", then what to do when it has no kernel.
pub(crate) fn found_note(found: &FoundEnvironment) -> String {
    let what = if found.file == found.label {
        format!("Found {}.", found.file)
    } else {
        format!("Found {} ({}).", found.file, found.label)
    };
    match &found.hint {
        Some(hint) => format!("{what} {hint}"),
        None => what,
    }
}

/// Whether to ask on opening: the project has no saved settings, and some
/// kernel is installed to pick.
pub(crate) fn worth_asking(setup: &ProjectSetup, specs: &[KernelspecInfo]) -> bool {
    !setup.saved && !specs.is_empty()
}

fn language_of(spec: &KernelspecInfo) -> String {
    spec.language.to_ascii_lowercase()
}

/// The dialog's lists that drop down.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Menu {
    R,
    Python,
    Documents,
    History,
}

pub(crate) struct SettingsDialog {
    setup: ProjectSetup,
    /// Asked unprompted, on opening the project: the way out is "Not now".
    first: bool,
    kernels: Vec<KernelspecInfo>,
    /// The kernel picked for each language; "" for automatic.
    r: String,
    python: String,
    documents: DocumentsRunIn,
    history: ChatHistory,
    menu: Option<Menu>,
    saving: bool,
    error: Option<String>,
}

/// The project's settings as last read, and the kernels it can pick from.
#[derive(Default)]
pub(crate) struct Setup {
    pub setup: Option<ProjectSetup>,
    pub kernels: Vec<KernelspecInfo>,
    pub dialog: Option<SettingsDialog>,
}

pub(crate) fn new_env_input(
    window: &mut Window,
    cx: &mut Context<Workbench>,
) -> (Entity<InputState>, Subscription) {
    let input = cx.new(|cx| InputState::new(window, cx).placeholder("none"));
    let subscription = cx.subscribe(&input, |this: &mut Workbench, _, event: &InputEvent, cx| {
        if let InputEvent::PressEnter { .. } = event {
            this.save_project_settings(cx);
        }
    });
    (input, subscription)
}

impl Workbench {
    /// Reads the project's settings and kernels; with `ask`, offers the
    /// dialog if the project has none saved yet (once per project).
    pub(crate) fn read_setup(&mut self, ask: bool, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(daemon), Some(project)) = (self.daemon(), self.project.clone()) else {
            return;
        };
        let setup = daemon.call::<ProjectSetup>(Call::GetProjectSettings {
            path: project.clone(),
        });
        let kernels = daemon.call::<Vec<KernelspecInfo>>(Call::ListProjectKernelspecs {
            path: project.clone(),
        });
        cx.spawn_in(window, async move |this, cx| {
            let (setup, kernels) = (setup.await, kernels.await);
            this.update_in(cx, |this, window, cx| {
                if this.project.as_deref() != Some(&project) {
                    return;
                }
                match setup {
                    Ok(setup) => {
                        this.setup.kernels = kernels.unwrap_or_else(|_| this.kernels.clone());
                        let offer = ask
                            && worth_asking(&setup, &this.setup.kernels)
                            && !crate::prefs::settings_asked(&project);
                        this.setup.setup = Some(setup);
                        if offer {
                            crate::prefs::save_settings_asked(&project);
                            this.open_settings_dialog(true, window, cx);
                        } else if !ask {
                            this.open_settings_dialog(false, window, cx);
                        }
                    }
                    Err(err) if !ask => {
                        this.problem =
                            Some(format!("Could not read the project's settings: {err:#}"))
                    }
                    Err(_) => {}
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The gear in the title bar: the settings, read again.
    pub(crate) fn show_project_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.read_setup(false, window, cx);
    }

    fn open_settings_dialog(&mut self, first: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(setup) = self.setup.setup.clone() else {
            return;
        };
        // A language starts as saved ("" for automatic), else as the
        // project runs now, so saving keeps it.
        let initial = |saved: &Option<String>, now: &Option<String>| {
            if setup.saved {
                saved.clone().unwrap_or_default()
            } else {
                now.clone().unwrap_or_default()
            }
        };
        let r = initial(&setup.settings.r, &setup.r);
        let python = initial(&setup.settings.python, &setup.python);
        let env = setup.settings.env_files.join(", ");
        self.settings_env
            .update(cx, |input, cx| input.set_value(env, window, cx));
        self.setup.dialog = Some(SettingsDialog {
            documents: setup.settings.documents,
            history: setup.settings.chat_history,
            setup,
            first,
            kernels: self.setup.kernels.clone(),
            r,
            python,
            menu: None,
            saving: false,
            error: None,
        });
        cx.notify();
    }

    pub(crate) fn close_settings_dialog(&mut self, cx: &mut Context<Self>) {
        self.setup.dialog = None;
        cx.notify();
    }

    pub(crate) fn save_project_settings(&mut self, cx: &mut Context<Self>) {
        let (Some(daemon), Some(project)) = (self.daemon(), self.project.clone()) else {
            return;
        };
        let env_files: Vec<String> = self
            .settings_env
            .read(cx)
            .value()
            .split(',')
            .map(|f| f.trim().to_string())
            .filter(|f| !f.is_empty())
            .collect();
        let Some(dialog) = &mut self.setup.dialog else {
            return;
        };
        let settings = ProjectSettings {
            r: Some(dialog.r.clone()).filter(|k| !k.is_empty()),
            python: Some(dialog.python.clone()).filter(|k| !k.is_empty()),
            documents: dialog.documents,
            env_files,
            chat_history: dialog.history,
        };
        dialog.saving = true;
        let save = daemon.call::<ProjectSetup>(Call::SaveProjectSettings {
            path: project.clone(),
            settings,
        });
        let running =
            self.active_session().is_some() || self.others.iter().any(|o| o.session.id().is_some());
        cx.spawn(async move |this, cx| {
            let saved = save.await;
            this.update(cx, |this, cx| {
                match saved {
                    Ok(setup) => {
                        this.setup.setup = Some(setup);
                        this.setup.dialog = None;
                        this.notice = Some(if running {
                            "Saved in .sandhill/project.toml. New sessions follow it; running ones keep their kernel until restarted.".into()
                        } else {
                            "Saved in .sandhill/project.toml.".into()
                        });
                        // The note goes after a while.
                        cx.spawn(async move |this, cx| {
                            cx.background_executor()
                                .timer(std::time::Duration::from_secs(8))
                                .await;
                            this.update(cx, |this, cx| {
                                this.notice = None;
                                cx.notify();
                            })
                            .ok();
                        })
                        .detach();
                    }
                    Err(err) => {
                        if let Some(dialog) = &mut this.setup.dialog {
                            dialog.saving = false;
                            dialog.error =
                                Some(format!("Could not save the project's settings: {err:#}"));
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

    /// The kernel the project runs `language` in, from its settings or the
    /// environment its files point to; none to pick as usual.
    pub(crate) fn project_kernel(&self, language: Option<&str>) -> Option<String> {
        let setup = self.setup.setup.as_ref()?;
        match language?.to_ascii_lowercase().as_str() {
            "r" => setup.r.clone(),
            "python" => setup.python.clone(),
            _ => None,
        }
    }

    /// Whether notebooks and Quarto documents run in the project's folder
    /// rather than their own.
    pub(crate) fn documents_in_project(&self) -> bool {
        self.setup
            .setup
            .as_ref()
            .is_some_and(|s| s.settings.documents == DocumentsRunIn::Project)
    }

    pub(crate) fn render_settings_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = self.setup.dialog.as_ref()?;
        let p = *Palette::get(cx);
        let name = self
            .project
            .as_deref()
            .and_then(|path| path.rsplit('/').next())
            .unwrap_or("the project")
            .to_string();
        let note = |text: String, warn: bool| {
            div()
                .text_size(px(look::SMALL))
                .text_color(if warn { p.warn } else { p.faint })
                .whitespace_normal()
                .child(text)
        };
        let label = |text: &'static str| {
            div()
                .w(px(110.))
                .flex_none()
                .pt(px(4.))
                .text_color(p.muted)
                .child(text)
        };
        let row = |label_el: Div, field: Div| {
            h_flex()
                .items_start()
                .gap(px(12.))
                .child(label_el)
                .child(field)
        };
        let mut rows: Vec<AnyElement> = Vec::new();
        for (menu, id, title) in [(Menu::R, "r", "R"), (Menu::Python, "python", "Python")] {
            let specs: Vec<&KernelspecInfo> = dialog
                .kernels
                .iter()
                .filter(|k| language_of(k) == id)
                .collect();
            let chosen = if id == "r" { &dialog.r } else { &dialog.python };
            let found: Vec<&FoundEnvironment> = dialog
                .setup
                .found
                .iter()
                .filter(|f| f.language == id)
                .collect();
            let automatic = {
                let found_kernel = found.iter().find(|f| f.kernel.is_some());
                let spec = match found_kernel {
                    Some(f) => dialog
                        .kernels
                        .iter()
                        .find(|k| Some(&k.name) == f.kernel.as_ref()),
                    None => specs.first().copied(),
                };
                let shown = spec
                    .map(|s| s.display_name.clone())
                    .or_else(|| found_kernel.and_then(|f| f.kernel.clone()));
                match (shown, found_kernel) {
                    (None, _) => "none installed".to_string(),
                    (Some(n), Some(f)) if !n.contains(&f.file) => format!("{n}, from {}", f.file),
                    (Some(n), _) => n,
                }
            };
            let mut options: Vec<(String, String)> =
                vec![(String::new(), format!("Automatic: {automatic}"))];
            options.extend(
                specs
                    .iter()
                    .map(|k| (k.name.clone(), k.display_name.clone())),
            );
            // A saved kernel this machine doesn't have stays listed, so it shows.
            if !chosen.is_empty() && !specs.iter().any(|k| &k.name == chosen) {
                options.push((chosen.clone(), format!("{chosen} (not installed here)")));
            }
            let mut field = v_flex()
                .flex_1()
                .min_w_0()
                .gap(px(4.))
                .child(self.select(menu, dialog, options, chosen, cx));
            for f in &found {
                field = field.child(note(found_note(f), f.hint.is_some()));
            }
            if specs.len() < 2 {
                field = field.child(note(only_kernel(id).into(), false));
            }
            if id == "r" && found.iter().any(|f| f.file == "renv.lock") {
                field = field.child(note(
                    "R starts in the project's folder, so renv loads the project's library.".into(),
                    false,
                ));
            }
            rows.push(row(label(title), field).into_any_element());
        }
        let documents = match dialog.documents {
            DocumentsRunIn::Document => "document",
            DocumentsRunIn::Project => "project",
        };
        rows.push(
            row(
                label("Notebooks"),
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(4.))
                    .child(self.select(
                        Menu::Documents,
                        dialog,
                        vec![
                            ("document".into(), "Run in their own folder, as quarto render does".into()),
                            ("project".into(), "Run in the project's folder".into()),
                        ],
                        documents,
                        cx,
                    ))
                    .child(note(
                        "For .ipynb, .qmd and .Rmd. The Console and scripts run in the project's folder.".into(),
                        false,
                    )),
            )
            .into_any_element(),
        );
        rows.push(
            row(
                label("Env files"),
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(4.))
                    .child(Input::new(&self.settings_env).small())
                    .child(note(
                        "NAME=value lines new R and Python sessions start with, comma-separated. Missing files are skipped.".into(),
                        false,
                    )),
            )
            .into_any_element(),
        );
        let history = match dialog.history {
            ChatHistory::Project => "project",
            ChatHistory::Machine => "machine",
        };
        rows.push(
            row(
                label("Chat history"),
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(4.))
                    .child(self.select(
                        Menu::History,
                        dialog,
                        vec![
                            ("project".into(), "Kept with the project".into()),
                            ("machine".into(), "Kept on this machine only".into()),
                        ],
                        history,
                        cx,
                    ))
                    .child(note(
                        if dialog.history == ChatHistory::Project {
                            "Agent conversations are saved in .sandhill/agents/, so they go with the folder to another computer and go on there. Git ignores them.".into()
                        } else {
                            "Agent conversations stay on the computer the project is on.".into()
                        },
                        false,
                    )),
            )
            .into_any_element(),
        );
        let card = v_flex()
            .id("settings-card")
            .occlude()
            .w(px(620.))
            .max_h(relative(0.9))
            .overflow_y_scroll()
            .gap(px(14.))
            .p(px(20.))
            .rounded(px(look::RADIUS))
            .bg(p.panel)
            .border_1()
            .border_color(p.glass_edge)
            .shadow_lg()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .text_size(px(16.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(if dialog.first {
                        format!("Which R and Python should {name} use?")
                    } else {
                        format!("Project settings · {name}")
                    }),
            )
            .child(note(
                "Saved in .sandhill/project.toml in the project's folder. Commit it to share these with collaborators.".into(),
                false,
            ))
            .when_some(dialog.setup.problem.clone(), |d, problem| {
                d.child(div().text_color(p.err).child(format!("Couldn't read the file: {problem}")))
            })
            .children(rows)
            .when_some(dialog.error.clone(), |d, err| d.child(div().text_color(p.err).child(err)))
            .child(
                h_flex()
                    .justify_end()
                    .gap(px(8.))
                    .child(
                        Button::new("settings-cancel")
                            .small()
                            .label(if dialog.first { "Not now" } else { "Cancel" })
                            .on_click(cx.listener(|this, _, _, cx| this.close_settings_dialog(cx))),
                    )
                    .child(
                        Button::new("settings-save")
                            .small()
                            .primary()
                            .label("Save")
                            .disabled(dialog.saving)
                            .on_click(cx.listener(|this, _, _, cx| this.save_project_settings(cx))),
                    ),
            );
        Some(
            deferred(
                div()
                    .id("settings-backdrop")
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(p.shadow.opacity(0.35))
                    .child(card)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| this.close_settings_dialog(cx)),
                    ),
            )
            .with_priority(2)
            .into_any_element(),
        )
    }

    /// A list that drops down under its button, as a `<select>`.
    fn select(
        &self,
        menu: Menu,
        dialog: &SettingsDialog,
        options: Vec<(String, String)>,
        chosen: &str,
        cx: &mut Context<Self>,
    ) -> Div {
        let p = *Palette::get(cx);
        let shown = options
            .iter()
            .find(|(value, _)| value == chosen)
            .map(|(_, label)| label.clone())
            .unwrap_or_default();
        let open = dialog.menu == Some(menu);
        let hover = p.ink(8.);
        let button = h_flex()
            .id(SharedString::from(format!("select-{}", menu as u8)))
            .h(px(28.))
            .px(px(10.))
            .gap(px(6.))
            .rounded(px(6.))
            .border_1()
            .border_color(if open { p.accent } else { p.line })
            .bg(p.bg)
            .cursor_pointer()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(shown),
            )
            .child(icon("chevron.down").size(px(12.)).text_color(p.muted))
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(dialog) = &mut this.setup.dialog {
                    dialog.menu = if dialog.menu == Some(menu) {
                        None
                    } else {
                        Some(menu)
                    };
                }
                cx.notify();
            }));
        let list = open.then(|| {
            v_flex()
                .rounded(px(6.))
                .border_1()
                .border_color(p.line)
                .bg(p.bg)
                .py(px(3.))
                .children(options.into_iter().enumerate().map(|(i, (value, label))| {
                    let on = value == chosen;
                    h_flex()
                        .id(SharedString::from(format!("select-{}-{i}", menu as u8)))
                        .h(px(26.))
                        .px(px(10.))
                        .gap(px(6.))
                        .cursor_pointer()
                        .hover(move |d| d.bg(hover))
                        .child(div().w(px(14.)).flex_none().when(on, |d| {
                            d.child(icon("check").size(px(12.)).text_color(p.accent))
                        }))
                        .child(
                            div()
                                .min_w_0()
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .child(label),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(dialog) = &mut this.setup.dialog {
                                match menu {
                                    Menu::R => dialog.r = value.clone(),
                                    Menu::Python => dialog.python = value.clone(),
                                    Menu::Documents => {
                                        dialog.documents = if value == "project" {
                                            DocumentsRunIn::Project
                                        } else {
                                            DocumentsRunIn::Document
                                        }
                                    }
                                    Menu::History => {
                                        dialog.history = if value == "machine" {
                                            ChatHistory::Machine
                                        } else {
                                            ChatHistory::Project
                                        }
                                    }
                                }
                                dialog.menu = None;
                            }
                            cx.notify();
                        }))
                }))
        });
        v_flex().gap(px(2.)).child(button).children(list)
    }
}

#[cfg(test)]
mod tests {
    use super::{found_note, worth_asking};
    use sandhill_proto::{FoundEnvironment, KernelspecInfo, ProjectSettings, ProjectSetup};

    #[test]
    fn notes_what_the_folder_suggests() {
        let found = FoundEnvironment {
            language: "python".into(),
            file: "environment.yml".into(),
            label: "conda env omicverse".into(),
            kernel: None,
            hint: Some("Install ipykernel in it.".into()),
        };
        assert_eq!(
            found_note(&found),
            "Found environment.yml (conda env omicverse). Install ipykernel in it."
        );
        let renv = FoundEnvironment {
            language: "r".into(),
            file: "renv.lock".into(),
            label: "renv.lock".into(),
            kernel: Some("ark".into()),
            hint: None,
        };
        assert_eq!(found_note(&renv), "Found renv.lock.");
        let setup = ProjectSetup {
            settings: ProjectSettings::default(),
            saved: false,
            problem: None,
            found: Vec::new(),
            r: None,
            python: None,
        };
        let spec: KernelspecInfo = serde_json::from_value(serde_json::json!({
            "name": "python3", "display_name": "Python 3", "language": "python"
        }))
        .unwrap();
        assert!(worth_asking(&setup, &[spec]));
        assert!(!worth_asking(&setup, &[]));
    }
}
