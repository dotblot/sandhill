//! Variables' Import button, as the Tauri app has it (`ImportDialog.tsx`):
//! a menu of ways to read a dataset into the session, then a dialog to pick
//! the file, name the variable and see the code before it runs. The code
//! runs in the session like any other, so it shows in the Console.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Disableable, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use sandhill_proto::{Call, DirEntry, EntryKind, RunStarted};

use crate::imports::{code_path, importable, imports_for, valid_variable, variable_name};
use crate::look::{self, Palette, bar_button, icon, picture};
use crate::workbench::Workbench;

pub(crate) struct ImportDialog {
    /// Which of the language's imports.
    kind: usize,
    language: String,
    session: String,
    /// The session's folder, which paths in the code are relative to.
    cwd: Option<String>,
    dir: String,
    entries: Option<Result<Vec<DirEntry>, String>>,
    file: Option<String>,
    running: bool,
    error: Option<String>,
    generation: u64,
}

#[derive(Default)]
pub(crate) struct Import {
    pub menu: bool,
    pub dialog: Option<ImportDialog>,
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

fn join(dir: &str, name: &str) -> String {
    if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

fn parent(dir: &str) -> String {
    match dir.trim_end_matches('/').rsplit_once('/') {
        Some(("", _)) | None => "/".into(),
        Some((up, _)) => up.into(),
    }
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
        let ready = self.active_session().is_some()
            && self
                .active_language()
                .is_some_and(|l| !imports_for(&l).is_empty());
        let tip = match self.active_language() {
            Some(l) if ready => format!("Import a dataset into {}", language_name(&l)),
            _ => "Start R or Python to import a dataset".into(),
        };
        // An icon only, beside the Variables filter.
        bar_button("import", "", Some("table"), &p)
            .px(px(5.))
            .gap(px(2.))
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
        let hover = p.ink(8.);
        let mut rows: Vec<AnyElement> = vec![
            div()
                .px(px(8.))
                .py(px(4.))
                .text_size(px(look::SMALL))
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
                        .my(px(4.))
                        .h(px(0.5))
                        .bg(p.glass_edge)
                        .into_any_element(),
                );
            }
            rows.push(
                h_flex()
                    .id(("import-kind", i))
                    .h(px(26.))
                    .px(px(8.))
                    .gap(px(16.))
                    .rounded(px(look::RADIUS - 4.))
                    .cursor_pointer()
                    .hover(move |d| d.bg(hover))
                    .child(div().flex_1().whitespace_nowrap().child(kind.label))
                    .child(
                        div()
                            .flex_none()
                            .font_family(look::MONO)
                            .text_size(px(look::SMALL))
                            .text_color(p.faint)
                            .child(kind.function),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.import.menu = false;
                        this.open_import(i, window, cx);
                    }))
                    .into_any_element(),
            );
        }
        let body = crate::composer_view::popup(&p)
            .id("import-menu")
            .occlude()
            .min_w(px(300.))
            .children(rows)
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
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

    fn open_import(&mut self, kind: usize, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(session), Some(language)) = (
            self.active_session().map(str::to_string),
            self.active_language(),
        ) else {
            return;
        };
        let cwd = self.session_folder();
        let dir = cwd
            .clone()
            .or_else(|| self.project.clone())
            .unwrap_or_else(|| "/".into());
        self.import_name
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.import.dialog = Some(ImportDialog {
            kind,
            language,
            session,
            cwd,
            dir: dir.clone(),
            entries: None,
            file: None,
            running: false,
            error: None,
            generation: 0,
        });
        self.list_import_dir(dir, window, cx);
    }

    fn list_import_dir(&mut self, dir: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(daemon) = self.daemon() else { return };
        let Some(dialog) = &mut self.import.dialog else {
            return;
        };
        dialog.generation += 1;
        dialog.dir = dir.clone();
        dialog.entries = None;
        dialog.file = None;
        let generation = dialog.generation;
        // For 10x output the folder itself is what's read.
        let kind = &imports_for(&dialog.language)[dialog.kind];
        if kind.folder {
            let name = variable_name(&dir);
            self.import_name
                .update(cx, |input, cx| input.set_value(name, window, cx));
        }
        let listed = daemon.call::<Vec<DirEntry>>(Call::ListDir {
            path: dir,
            sizes: false,
        });
        cx.spawn(async move |this, cx| {
            let listed = listed.await;
            this.update(cx, |this, cx| {
                let Some(dialog) = &mut this.import.dialog else {
                    return;
                };
                if dialog.generation != generation {
                    return;
                }
                let kind = &imports_for(&dialog.language)[dialog.kind];
                dialog.entries = Some(
                    listed
                        .map(|entries| {
                            entries
                                .into_iter()
                                .filter(|e| {
                                    !e.name.starts_with('.')
                                        && (e.kind == EntryKind::Dir
                                            || (e.kind == EntryKind::File
                                                && importable(kind, &e.name)))
                                })
                                .collect()
                        })
                        .map_err(|err| format!("{err:#}")),
                );
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn pick_import_file(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = &mut self.import.dialog else {
            return;
        };
        dialog.file = Some(path.clone());
        let name = variable_name(&path);
        self.import_name.update(cx, |input, cx| {
            input.set_value(name, window, cx);
            input.focus(window, cx);
        });
        cx.notify();
    }

    /// What the dialog would run now, and whether the name is a bad one.
    fn import_code(&self, cx: &App) -> Option<(String, bool)> {
        let dialog = self.import.dialog.as_ref()?;
        let kind = &imports_for(&dialog.language)[dialog.kind];
        let chosen = dialog
            .file
            .clone()
            .or_else(|| kind.folder.then(|| dialog.dir.clone()))?;
        let typed = self.import_name.read(cx).value().trim().to_string();
        let named = kind.named(&chosen);
        let bad = named && !typed.is_empty() && !valid_variable(&typed, &dialog.language);
        let name = if typed.is_empty() {
            "data".to_string()
        } else {
            typed
        };
        Some((
            kind.code(&code_path(&chosen, dialog.cwd.as_deref()), &name),
            bad,
        ))
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
        let started = daemon.call_in_order::<RunStarted>(Call::Execute {
            session: dialog.session.clone(),
            code,
            cell: None,
            quiet: false,
            by: None,
        });
        cx.spawn(async move |this, cx| {
            let started = started.await;
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

    /// Pick the file (or 10x folder), name the variable, see the code, run
    /// it.
    pub(crate) fn render_import_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let dialog = self.import.dialog.as_ref()?;
        let p = *Palette::get(cx);
        let kind = &imports_for(&dialog.language)[dialog.kind];
        let hover = p.ink(8.);
        let list: AnyElement = match &dialog.entries {
            None => div()
                .p(px(8.))
                .text_color(p.faint)
                .child("Loading…")
                .into_any_element(),
            Some(Err(err)) => div()
                .p(px(8.))
                .text_color(p.err)
                .child(err.clone())
                .into_any_element(),
            Some(Ok(entries)) if entries.is_empty() => div()
                .p(px(8.))
                .text_color(p.faint)
                .child(if kind.folder {
                    "No folders or .h5 files here"
                } else {
                    "No files it reads here"
                })
                .into_any_element(),
            Some(Ok(entries)) => v_flex()
                .children(entries.iter().enumerate().map(|(i, entry)| {
                    let path = join(&dialog.dir, &entry.name);
                    let dir = entry.kind == EntryKind::Dir;
                    let on = dialog.file.as_deref() == Some(path.as_str());
                    h_flex()
                        .id(("import-entry", i))
                        .h(px(26.))
                        .px(px(8.))
                        .gap(px(6.))
                        .rounded(px(4.))
                        .cursor_pointer()
                        .when(on, |d| d.bg(p.sel))
                        .when(!on, |d| d.hover(move |d| d.bg(hover)))
                        .child(
                            picture(&if dir {
                                look::folder_icon(&entry.name)
                            } else {
                                look::file_icon(&entry.name)
                            })
                            .size(px(15.)),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .child(entry.name.clone()),
                        )
                        .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                            if dir {
                                this.list_import_dir(path.clone(), window, cx);
                            } else if event.click_count() >= 2 {
                                this.pick_import_file(path.clone(), window, cx);
                                this.run_import(cx);
                            } else {
                                this.pick_import_file(path.clone(), window, cx);
                            }
                        }))
                }))
                .into_any_element(),
        };
        let code = self.import_code(cx);
        let named = dialog
            .file
            .as_deref()
            .or(kind.folder.then_some(dialog.dir.as_str()))
            .is_none_or(|path| kind.named(path));
        let bad = code.as_ref().is_some_and(|(_, bad)| *bad);
        let at_root = dialog.dir == "/";
        let dir = dialog.dir.clone();
        let card = v_flex()
            .id("import-card")
            .occlude()
            .w(px(560.))
            .max_h(relative(0.9))
            .gap(px(12.))
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
                    .child(format!(
                        "{} into {}",
                        kind.label.trim_end_matches('…'),
                        language_name(&dialog.language)
                    )),
            )
            .child(
                v_flex()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(p.line)
                    .child(
                        h_flex()
                            .h(px(30.))
                            .px(px(4.))
                            .gap(px(6.))
                            .border_b_1()
                            .border_color(p.line)
                            .child(
                                look::icon_button("import-up", "arrow.up", &p, false)
                                    .when(at_root, |d| d.opacity(0.35))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        if !at_root {
                                            this.list_import_dir(parent(&dir), window, cx);
                                        }
                                    })),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .text_color(p.muted)
                                    .child(dialog.dir.clone()),
                            ),
                    )
                    .child(
                        div()
                            .id("import-list")
                            .h(px(240.))
                            .overflow_y_scroll()
                            .p(px(4.))
                            .child(list),
                    ),
            )
            .when(named, |d| {
                d.child(
                    h_flex()
                        .gap(px(10.))
                        .child(div().w(px(50.)).text_color(p.muted).child("Name"))
                        .child(div().flex_1().child(Input::new(&self.import_name).small())),
                )
                .when(bad, |d| {
                    d.child(div().text_color(p.err).child(format!(
                        "That isn't a {} variable name",
                        language_name(&dialog.language)
                    )))
                })
            })
            .child(
                div()
                    .p(px(10.))
                    .rounded(px(6.))
                    .bg(p.chunk)
                    .font_family(look::MONO)
                    .text_size(px(look::CODE - 1.))
                    .whitespace_normal()
                    .child(match &code {
                        Some((code, _)) => code.clone(),
                        None if kind.folder => {
                            "Open the folder with the 10x output, or pick an .h5 file".into()
                        }
                        None => "Pick a file".into(),
                    }),
            )
            .when_some(dialog.error.clone(), |d, err| {
                d.child(div().text_color(p.err).child(err))
            })
            .child(
                h_flex()
                    .justify_end()
                    .gap(px(8.))
                    .child(
                        Button::new("import-cancel")
                            .small()
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.import.dialog = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("import-run")
                            .small()
                            .primary()
                            .label("Import")
                            .disabled(code.is_none() || bad || dialog.running)
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
}

#[cfg(test)]
mod tests {
    use super::{join, parent};

    #[test]
    fn walks_folders() {
        assert_eq!(join("/p", "data"), "/p/data");
        assert_eq!(join("/", "home"), "/home");
        assert_eq!(parent("/p/data"), "/p");
        assert_eq!(parent("/p"), "/");
        assert_eq!(parent("/"), "/");
    }
}
