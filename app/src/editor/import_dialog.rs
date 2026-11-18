//! Variables' Import button, as the Tauri app has it (`ImportDialog.tsx`):
//! a menu of ways to read a dataset into the session, then the file chosen
//! the way Open Folder chooses (the system's window on this Mac, Sandhill's
//! own chooser on servers), then a card to name the variable and see the
//! code before it runs. The code runs in the session like any other, so it
//! shows in the Console.

use gpui_kit::base::ElementExt as _;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Disableable, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use sandhill_proto::{Call, RunStarted, SessionInfo, SessionStatus};

use crate::editor::imports::{
    generated_import_code, importable, imports_for, valid_variable, variable_name,
};
use crate::look::{self, Palette, bar_button, icon, picture};
use crate::welcome::folder_dialog::{FolderDialog, FolderDialogDone, Place, choose_local_path};
use crate::workbench::Workbench;

#[derive(Clone)]
pub(crate) struct ImportDialog {
    /// Which of the language's imports.
    kind: usize,
    language: String,
    session: String,
    /// The session's folder, which paths in the code are relative to.
    cwd: Option<String>,
    /// The file (or 10x folder) chosen.
    file: String,
    /// Whether the import reads it; the system's window can't be told
    /// which files to offer.
    fits: bool,
    running: bool,
    error: Option<String>,
}

#[derive(Default)]
pub(crate) struct Import {
    pub menu: bool,
    pub dialog: Option<ImportDialog>,
    /// Sandhill's own chooser, for a session on a server.
    pub chooser: Option<(Entity<FolderDialog>, Subscription)>,
}

pub(crate) fn new_import_input(
    window: &mut Window,
    cx: &mut Context<Workbench>,
) -> (Entity<InputState>, Subscription) {
    let input = cx.new(|cx| InputState::new(window, cx));
    let subscription = cx.subscribe(&input, |this: &mut Workbench, _, event: &InputEvent, cx| {
        match event {
            InputEvent::PressEnter { .. } => this.run_import(cx),
            // The code shown follows the name typed.
            InputEvent::Change => cx.notify(),
            _ => {}
        }
    });
    (input, subscription)
}

fn language_name(language: &str) -> &'static str {
    if language == "r" { "R" } else { "Python" }
}

impl Workbench {
    /// The session's folder: a notebook's own (unless the project runs
    /// them in its folder), else the project's.
    fn session_folder(&self) -> Option<String> {
        match &self.open {
            Some(open) if !open.is_script() && !self.documents_in_project() => {
                Some(open.notebook.folder().to_string())
            }
            _ => self.project.clone(),
        }
    }

    pub(crate) fn import_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = *Palette::get(cx);
        let ready = self.variables_session().is_some()
            && self
                .active_language()
                .is_some_and(|l| !imports_for(&l).is_empty());
        let tip = match self.active_language() {
            Some(l) if ready => format!("Import a dataset into {}", language_name(&l)),
            _ => "Start R or Python to import a dataset".into(),
        };
        // An icon only, beside the Variables filter.
        bar_button("import", "", Some("data.import"), &p)
            .px(px(5.))
            .gap(px(2.))
            .on_prepaint(self.popup_triggers.track("import-kind"))
            .child(icon("chevron.down").size(px(11.)))
            .when(!ready, |d| d.opacity(0.4))
            .tooltip(move |w, cx| {
                gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(w, cx)
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                if ready {
                    this.import.menu = !this.import.menu;
                    cx.notify();
                }
            }))
            .into_any_element()
    }

    /// The menu of ways to read a dataset, under the Import button.
    pub(crate) fn render_import_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.import.menu {
            return None;
        }
        let p = *Palette::get(cx);
        let language = self.active_language()?;
        let hover = p.ink(look::ROW_HOVER);
        let mut rows: Vec<AnyElement> = vec![
            div()
                .px(px(look::GAP_8))
                .py(px(look::GAP_4))
                .text_size(px(look::small()))
                .text_color(p.faint)
                .child(format!(
                    "Import a dataset into {}",
                    language_name(&language)
                ))
                .into_any_element(),
        ];
        for (i, kind) in imports_for(&language).iter().enumerate() {
            if kind.group {
                rows.push(
                    div()
                        .my(px(look::GAP_4))
                        .h(px(0.5))
                        .bg(p.glass_edge)
                        .into_any_element(),
                );
            }
            rows.push(
                h_flex()
                    .id(("import-kind", i))
                    .h(px(look::row()))
                    .px(px(look::GAP_8))
                    .gap(px(look::GAP_12))
                    .rounded(px(look::CONTROL_RADIUS))
                    .cursor_pointer()
                    .hover(move |d| d.bg(hover))
                    .child(div().flex_1().whitespace_nowrap().child(kind.label))
                    .child(
                        div()
                            .flex_none()
                            .font_family(look::mono())
                            .text_size(px(look::small()))
                            .text_color(p.faint)
                            .child(kind.function),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.import.menu = false;
                        this.choose_import_file(i, window, cx);
                    }))
                    .into_any_element(),
            );
        }
        let body = crate::agents::composer_view::popup(&p)
            .id("import-menu")
            .occlude()
            .min_w(px(300.))
            .children(rows)
            .on_mouse_down_out(cx.listener(|this, event: &MouseDownEvent, _, cx| {
                if this.popup_triggers.contains("import-kind", event.position) {
                    return;
                }
                this.import.menu = false;
                cx.notify();
            }));
        Some(
            div()
                .absolute()
                // Under the Import button, in the row at the top of Variables.
                .top(px(62.))
                .right(px(8.))
                .child(deferred(body).with_priority(3))
                .into_any_element(),
        )
    }

    /// Asks for the file to import as Open Folder asks for a folder: the
    /// system's window on this Mac, Sandhill's own chooser on a server.
    fn choose_import_file(&mut self, kind: usize, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(session), Some(language)) = (
            self.variables_session().map(str::to_string),
            self.active_language(),
        ) else {
            return;
        };
        let Some(daemon) = self.daemon() else { return };
        let reads = &imports_for(&language)[kind];
        let title = format!(
            "{} into {}",
            reads.label.trim_end_matches('…'),
            language_name(&language)
        );
        // Filled in with the chosen file.
        let asked = ImportDialog {
            kind,
            language,
            session,
            cwd: self.session_folder(),
            file: String::new(),
            fits: true,
            running: false,
            error: None,
        };
        if self.host() == sandhill_link::LOCAL {
            let chosen = choose_local_path("Choose", true, reads.folder, cx);
            cx.spawn_in(window, async move |this, cx| {
                let Some(file) = chosen.await else { return };
                let fits = importable(reads, &file)
                    || (reads.folder && std::path::Path::new(&file).is_dir());
                this.update_in(cx, |this, window, cx| {
                    this.open_import(
                        ImportDialog {
                            file,
                            fits,
                            ..asked
                        },
                        window,
                        cx,
                    )
                })
                .ok();
            })
            .detach();
            return;
        }
        let start = asked.cwd.clone().or_else(|| self.project.clone());
        let places = self
            .project
            .iter()
            .map(|project| Place {
                label: project
                    .rsplit('/')
                    .next()
                    .unwrap_or(project)
                    .to_string()
                    .into(),
                path: project.clone(),
            })
            .collect();
        let chooser = cx.new(|cx| {
            FolderDialog::new(
                daemon,
                title.into(),
                "Choose".into(),
                start,
                places,
                window,
                cx,
            )
            .files(move |name| importable(reads, name), reads.folder)
        });
        let subscription = cx.subscribe_in(
            &chooser,
            window,
            move |this, _, event: &FolderDialogDone, window, cx| {
                this.import.chooser = None;
                match &event.0 {
                    Some(file) => {
                        let file = file.clone();
                        this.open_import(
                            ImportDialog {
                                file,
                                ..asked.clone()
                            },
                            window,
                            cx,
                        )
                    }
                    None => cx.notify(),
                }
            },
        );
        self.import.chooser = Some((chooser, subscription));
        cx.notify();
    }

    /// Shows the card for a chosen file, its variable named after it.
    fn open_import(&mut self, dialog: ImportDialog, window: &mut Window, cx: &mut Context<Self>) {
        let name = variable_name(&dialog.file);
        self.import_name.update(cx, |input, cx| {
            input.set_value(name, window, cx);
            input.focus(window, cx);
        });
        self.import.dialog = Some(dialog);
        cx.notify();
    }

    pub(crate) fn open_preselected_import(
        &mut self,
        path: String,
        session: SessionInfo,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let language = session.language.to_ascii_lowercase();
        if session.status != SessionStatus::Running || imports_for(&language).is_empty() {
            self.problem = Some("Import requires an existing live R or Python session".into());
            cx.notify();
            return;
        }
        self.import.menu = false;
        let dialog = ImportDialog {
            kind: 0,
            language,
            session: session.id,
            cwd: Some(session.cwd),
            file: path,
            fits: true,
            running: false,
            error: None,
        };
        self.open_import(dialog, window, cx);
    }

    /// What the dialog would run now, and whether the name is a bad one.
    pub(crate) fn import_code(&self, cx: &App) -> Option<(String, bool)> {
        let dialog = self.import.dialog.as_ref()?;
        if !dialog.fits {
            return None;
        }
        let kind = &imports_for(&dialog.language)[dialog.kind];
        let chosen = dialog.file.clone();
        let typed = self.import_name.read(cx).value().trim().to_string();
        let named = kind.named(&chosen);
        let bad = named && !typed.is_empty() && !valid_variable(&typed, &dialog.language);
        let name = if typed.is_empty() {
            "data".to_string()
        } else {
            typed
        };
        Some((
            generated_import_code(
                &dialog.language,
                kind,
                &chosen,
                dialog.cwd.as_deref(),
                &name,
            ),
            bad,
        ))
    }

    pub(crate) fn copy_import_code(&mut self, cx: &mut Context<Self>) {
        let Some((code, false)) = self.import_code(cx) else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(code));
        self.notice = Some("Copied import code.".into());
        cx.notify();
    }

    pub(crate) fn insert_import_code(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((code, false)) = self.import_code(cx) else {
            return;
        };
        if self.insert_into_active_document(&code, window, cx) {
            self.import.dialog = None;
            cx.notify();
        }
    }

    pub(crate) fn run_import(&mut self, cx: &mut Context<Self>) {
        let Some(daemon) = self.daemon() else { return };
        let Some((code, false)) = self.import_code(cx) else {
            return;
        };
        let Some(dialog) = &mut self.import.dialog else {
            return;
        };
        if dialog.running {
            return;
        }
        dialog.running = true;
        dialog.error = None;
        let session = dialog.session.clone();
        cx.spawn(async move |this, cx| {
            let started = async {
                let sessions = daemon.call::<Vec<SessionInfo>>(Call::ListSessions).await?;
                let live = sessions
                    .iter()
                    .find(|s| s.id == session && s.status == SessionStatus::Running)
                    .ok_or_else(|| {
                        anyhow::anyhow!("The selected session has ended; choose a live session")
                    })?;
                if live.pending_runs > 0 {
                    anyhow::bail!(
                        "The selected session is busy; wait for it to finish and try again"
                    );
                }
                daemon
                    .call_in_order::<RunStarted>(Call::Execute {
                        session,
                        code,
                        cell: None,
                        quiet: false,
                        by: None,
                    })
                    .await
            }
            .await;
            this.update(cx, |this, cx| {
                match started {
                    Ok(_) => this.import.dialog = None,
                    Err(err) => {
                        if let Some(dialog) = &mut this.import.dialog {
                            dialog.running = false;
                            dialog.error = Some(format!("Could not import: {err:#}"));
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

    /// The chosen file (or 10x folder), the variable's name and the code,
    /// in a card styled as the server chooser's.
    pub(crate) fn render_import_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = self.import.dialog.as_ref()?;
        let p = *Palette::get(cx);
        let kind = &imports_for(&dialog.language)[dialog.kind];
        let code = self.import_code(cx);
        let named = kind.named(&dialog.file);
        let bad = code.as_ref().is_some_and(|(_, bad)| *bad);
        let blocked = code.is_none() || bad || dialog.running;
        let file_name = dialog
            .file
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("")
            .to_string();
        let mark = if dialog.fits && !importable(kind, &dialog.file) {
            look::folder_icon(&file_name)
        } else {
            look::file_icon(&file_name)
        };
        let again = dialog.kind;
        let button = |id: &'static str, label: &'static str| Button::new(id).small().label(label);
        let card = v_flex()
            .id("import-card")
            .occlude()
            .w(px(560.))
            .max_w(relative(0.9))
            .max_h(relative(0.9))
            .overflow_y_scroll()
            .gap(px(look::GAP_12))
            .p(px(20.))
            .rounded(px(look::RADIUS))
            .bg(p.panel)
            .border_1()
            .border_color(p.glass_edge)
            .shadow_lg()
            .text_color(p.text)
            .text_size(px(look::small()))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .text_size(px(look::title()))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(format!(
                        "{} into {}",
                        kind.label.trim_end_matches('…'),
                        language_name(&dialog.language)
                    )),
            )
            .child(
                h_flex()
                    .gap(px(look::GAP_8))
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(px(look::GAP_4))
                            .text_color(p.muted)
                            .font_family(look::mono())
                            .child(picture(&mark).size(px(look::ICON)).flex_none())
                            .child(div().min_w_0().truncate().child(dialog.file.clone())),
                    )
                    .child(
                        look::button("import-change", "Choose Another…", None, look::ButtonSize::Row)
                            .disabled(dialog.running)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.import.dialog = None;
                                this.choose_import_file(again, window, cx);
                            })),
                    ),
            )
            .when(!dialog.fits, |d| {
                d.child(div().text_color(p.err).child(format!(
                    "{} doesn't read {file_name}. Choose a {} file{}.",
                    kind.function,
                    kind.exts
                        .iter()
                        .filter(|ext| **ext != "gz")
                        .map(|ext| format!(".{ext}"))
                        .collect::<Vec<_>>()
                        .join(", "),
                    if kind.folder { ", or a folder of 10x output" } else { "" },
                )))
            })
            .when(dialog.fits && named, |d| {
                d.child(
                    h_flex()
                        .gap(px(look::GAP_8))
                        .child(div().w(px(50.)).text_color(p.muted).child("Name"))
                        .child(div().flex_1().child(Input::new(&self.import_name).small())),
                )
                .when(bad, |d| {
                    d.child(div().text_color(p.err).child(format!(
                        "That isn't a {} variable name",
                        language_name(&dialog.language)
                    )))
                })
                .child(div().text_color(p.muted).child(
                    "Existing variables are protected. Choose a new name if this name is already in use.",
                ))
            })
            .when(dialog.fits && !named, |d| {
                d.child(div().text_color(p.warn).child(
                    "Loading R data can replace existing variables with the same names.",
                ))
            })
            .when_some(code.as_ref().map(|(code, _)| code.clone()), |d, code| {
                d.child(
                    div()
                        .p(px(look::GAP_8))
                        .rounded(px(look::CODE_RADIUS))
                        .bg(p.chunk)
                        .font_family(look::mono())
                        .text_size(px(look::code()))
                        .whitespace_normal()
                        .child(code),
                )
            })
            .when_some(dialog.error.clone(), |d, err| {
                d.child(div().text_color(p.err).child(err))
            })
            .child(
                h_flex()
                    .justify_end()
                    .gap(px(look::GAP_8))
                    .flex_wrap()
                    .child(
                        button("import-copy", "Copy Code")
                            .disabled(blocked)
                            .on_click(cx.listener(|this, _, _, cx| this.copy_import_code(cx))),
                    )
                    .child(
                        button("import-insert", "Insert into Document")
                            .disabled(blocked || !self.can_insert_into_active_document(cx))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.insert_import_code(window, cx)
                            })),
                    )
                    .child(button("import-cancel", "Cancel").on_click(cx.listener(
                        |this, _, _, cx| {
                            this.import.dialog = None;
                            cx.notify();
                        },
                    )))
                    .child(
                        button("import-run", "Import")
                            .primary()
                            .disabled(blocked)
                            .on_click(cx.listener(|this, _, _, cx| this.run_import(cx))),
                    ),
            );
        Some(
            deferred(
                div()
                    .id("import-backdrop")
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(p.shadow.opacity(0.35))
                    .child(card)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.import.dialog = None;
                            cx.notify();
                        }),
                    ),
            )
            .with_priority(2)
            .into_any_element(),
        )
    }

    /// Sandhill's own chooser, while it asks which file to import.
    pub(crate) fn render_import_chooser(&self) -> Option<Entity<FolderDialog>> {
        self.import
            .chooser
            .as_ref()
            .map(|(chooser, _)| chooser.clone())
    }
}
