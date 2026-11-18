//! ⌘K, as in the Tauri app's `Palette.tsx`: go to a file of the project,
//! or run a command, by typing part of its name. With nothing typed it
//! offers the first few documents, then every command.

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use sandhill_proto::{Call, DirEntry, EntryKind};

use crate::look::{self, Palette, icon, picture};
use crate::panes::files::{MAX_DIRS, MAX_FILES, NewKind, SKIP, match_score};
use crate::welcome::start_screen::StartMode;
use crate::workbench::{ConsoleTab, Workbench};

/// How many matches show at most.
const SHOWN: usize = 50;

/// What a command does. The menu bar, keys and palette share these.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Cmd {
    Save,
    SaveAll,
    CloseTab,
    FileHistory,
    RunAll,
    Preview,
    RenderDocument,
    RunAsJob,
    Interrupt,
    New(NewKind),
    ToggleFiles,
    ToggleMiddle,
    ToggleConsole,
    NotebookConsole,
    NotebookPlots,
    ToggleAgents,
    ToggleRight,
    FocusEditor,
    OpenTerminal,
    ShowJobs,
    RefreshFiles,
    OpenProject,
    Connect,
    ProjectSettings,
    Settings,
    CloseProject,
}

pub(crate) struct Command {
    pub label: String,
    pub keys: &'static str,
    pub cmd: Cmd,
}

#[derive(Clone)]
enum Choice {
    /// A file, by its path in the project.
    File(String),
    Cmd(Cmd),
}

struct Item {
    label: String,
    detail: Option<String>,
    choice: Choice,
}

#[derive(Default)]
pub(crate) struct CommandPalette {
    pub shown: bool,
    index: usize,
    /// The project's documents, by their path in it, once listed.
    files: Option<Vec<String>>,
    generation: u64,
    scroll: ScrollHandle,
}

pub(crate) fn new_palette_input(
    window: &mut Window,
    cx: &mut Context<Workbench>,
) -> (Entity<InputState>, Subscription) {
    let input =
        cx.new(|cx| InputState::new(window, cx).placeholder("Go to a file or run a command"));
    let subscription = cx.subscribe_in(
        &input,
        window,
        |this: &mut Workbench, _, event: &InputEvent, window, cx| match event {
            InputEvent::PressEnter { .. } => {
                let ix = this.palette.index;
                this.palette_choose(ix, window, cx);
            }
            InputEvent::Change => {
                this.palette.index = 0;
                this.palette.scroll.scroll_to_item(0);
                cx.notify();
            }
            _ => {}
        },
    );
    (input, subscription)
}

impl Workbench {
    /// What ⌘K offers besides files, as the Tauri app lists them.
    pub(crate) fn commands(&self) -> Vec<Command> {
        let c = |label: &str, keys: &'static str, cmd: Cmd| Command {
            label: label.to_string(),
            keys,
            cmd,
        };
        let mut list = Vec::new();
        if self.objects.is_shown()
            || self.explorer_shown
            || self.diff_shown
            || self.history_shown
            || self.media_shown.is_some()
        {
            list.push(c("Close tab", "⌘W", Cmd::CloseTab));
        } else if let Some(open) = &self.open {
            list.push(c("Save", "⌘S", Cmd::Save));
            list.push(c("Save all", "⌥⌘S", Cmd::SaveAll));
            list.push(c("Close tab", "⌘W", Cmd::CloseTab));
            if !open.notebook.truncated {
                list.push(c("File History…", "", Cmd::FileHistory));
            }
            let run = if open.is_script() {
                "Run file"
            } else {
                "Run all"
            };
            list.push(c(run, "⌥⌘R", Cmd::RunAll));
            if crate::panes::viewer::is_previewable(&open.notebook.path) {
                list.push(c("Quarto Preview", "⇧⌘K", Cmd::Preview));
                list.push(c("Render Document", "", Cmd::RenderDocument));
            }
            if crate::panes::jobs::is_job_script(&open.notebook.path) {
                list.push(c("Run as job", "", Cmd::RunAsJob));
            }
            if let Some(language) = self.active_language() {
                let name = if language == "r" { "R" } else { "Python" };
                list.push(c(&format!("{name}: Interrupt"), "⌘.", Cmd::Interrupt));
            }
        }
        for kind in NewKind::ALL {
            list.push(c(
                &format!("{}…", kind.label()),
                kind.keys(),
                Cmd::New(kind),
            ));
        }
        let show = |on: bool, what: &str| format!("{} {what}", if on { "Hide" } else { "Show" });
        list.push(c(
            &show(self.show_files, "left panel"),
            "⌘B",
            Cmd::ToggleFiles,
        ));
        list.push(c(
            &show(self.show_middle, "editor column"),
            "",
            Cmd::ToggleMiddle,
        ));
        list.push(c(
            &show(self.show_console, "console"),
            "⌘J",
            Cmd::ToggleConsole,
        ));
        list.push(c(
            if self.notebook_console {
                "Keep notebook runs out of the Console"
            } else {
                "Show notebook runs in the Console"
            },
            "",
            Cmd::NotebookConsole,
        ));
        list.push(c(
            if self.notebook_plots {
                "Keep notebook plots out of Plots"
            } else {
                "Show notebook plots in Plots"
            },
            "",
            Cmd::NotebookPlots,
        ));
        list.push(c(
            &show(self.show_agents, "AI panel"),
            "⌘I",
            Cmd::ToggleAgents,
        ));
        list.push(c(
            if self.focus_mode.active() {
                "Focus Editor: Restore layout"
            } else {
                "Focus Editor"
            },
            "⌃⌘↩",
            Cmd::FocusEditor,
        ));
        list.push(c("Open terminal", "", Cmd::OpenTerminal));
        list.push(c("Show jobs", "", Cmd::ShowJobs));
        list.push(c("Refresh files", "", Cmd::RefreshFiles));
        if self.project.is_some() {
            list.push(c("Project settings…", "", Cmd::ProjectSettings));
            list.push(c("Close project", "", Cmd::CloseProject));
        }
        list.push(c("Settings…", "⌘,", Cmd::Settings));
        list.push(c("Open project…", "⌘O", Cmd::OpenProject));
        list.push(c("Connect to server…", "⇧⌘O", Cmd::Connect));
        list
    }

    /// Does what a command says, from the palette, a menu or a key.
    pub(crate) fn run_command(&mut self, cmd: Cmd, window: &mut Window, cx: &mut Context<Self>) {
        match cmd {
            Cmd::Save => self.save(cx),
            Cmd::SaveAll => self.save_all(cx),
            Cmd::FileHistory => {
                if let Some(path) = self.open.as_ref().map(|open| open.notebook.path.clone()) {
                    self.open_file_history(path, cx);
                }
            }
            Cmd::CloseTab => {
                if self.objects.is_shown() {
                    self.close_shown_object_explorer(cx)
                } else if self.explorer_shown {
                    self.close_explorer(cx)
                } else if self.diff_shown {
                    self.close_diff(cx)
                } else if self.history_shown {
                    self.close_file_history(cx)
                } else if let Some(path) = self.active_document_path().map(str::to_string) {
                    self.ask_close(path, window, cx)
                }
            }
            Cmd::RunAll => self.run_all(cx),
            Cmd::Preview => {
                if let Some(path) = self
                    .open
                    .as_ref()
                    .map(|o| o.notebook.path.clone())
                    .filter(|p| crate::panes::viewer::is_previewable(p))
                {
                    self.preview_document(path, cx);
                }
            }
            Cmd::RenderDocument => self.render_document(cx),
            Cmd::RunAsJob => self.run_open_as_job(cx),
            Cmd::Interrupt => self.interrupt(),
            Cmd::New(kind) => self.start_new(kind, None, window, cx),
            Cmd::ToggleFiles => {
                self.toggle_column(crate::shell::pane_sizes::Panel::LeftTop, window, cx)
            }
            Cmd::ToggleMiddle => {
                self.toggle_column(crate::shell::pane_sizes::Panel::Editor, window, cx)
            }
            Cmd::ToggleConsole => {
                if !self.show_middle || !self.show_console {
                    self.reveal_middle();
                    self.show_console = true;
                } else {
                    self.show_console = false;
                    if self.maximized_panel == Some(crate::shell::pane_sizes::Panel::Console) {
                        self.maximized_panel = None;
                    }
                }
            }
            Cmd::NotebookConsole => crate::shell::choices::choose(
                crate::shell::choices::Choice::NotebookConsole(!self.notebook_console),
                cx,
            ),
            Cmd::NotebookPlots => crate::shell::choices::choose(
                crate::shell::choices::Choice::NotebookPlots(!self.notebook_plots),
                cx,
            ),
            Cmd::ToggleAgents => {
                self.toggle_column(crate::shell::pane_sizes::Panel::Agents, window, cx)
            }
            // ⌥⌘B, which showed the right column, now shows the AI pane
            // that took its place.
            Cmd::ToggleRight => {
                self.toggle_column(crate::shell::pane_sizes::Panel::Agents, window, cx)
            }
            Cmd::FocusEditor => self.toggle_focus_editor(cx),
            Cmd::OpenTerminal => self.show_tab(ConsoleTab::Terminal, window, cx),
            Cmd::ShowJobs => self.show_tab(ConsoleTab::Jobs, window, cx),
            Cmd::RefreshFiles => self.refresh_files(cx),
            Cmd::OpenProject => {
                let host = self.host();
                self.with_shell(window, cx, move |shell, window, cx| {
                    shell.show_start(StartMode::Picking(host), window, cx)
                });
            }
            Cmd::Connect => self.with_shell(window, cx, |shell, window, cx| {
                shell.show_start(StartMode::Adding, window, cx)
            }),
            Cmd::ProjectSettings => self.show_project_settings(window, cx),
            Cmd::Settings => self.with_shell(window, cx, |shell, window, cx| {
                shell.show_start(StartMode::General, window, cx)
            }),
            Cmd::CloseProject => {
                let id = cx.entity_id();
                self.with_shell(window, cx, move |shell, window, cx| {
                    shell.close_tab(id, window, cx)
                });
            }
        }
        cx.notify();
    }

    pub(crate) fn toggle_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.shown {
            self.close_palette(window, cx);
            return;
        }
        self.palette.shown = true;
        self.palette.index = 0;
        self.palette.files = None;
        self.palette.generation += 1;
        self.palette_input.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.focus(window, cx);
        });
        self.list_project_files(cx);
        cx.notify();
    }

    fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.palette.shown = false;
        window.blur(cx);
        cx.notify();
    }

    /// The project's documents (.qmd, .Rmd, .ipynb, .R, .py, .html),
    /// breadth first, skipping hidden folders and library folders, and
    /// stopping after a few hundred folders so a huge data folder can't
    /// stall it.
    fn list_project_files(&mut self, cx: &mut Context<Self>) {
        let (Some(daemon), Some(project)) = (self.daemon(), self.project.clone()) else {
            self.palette.files = Some(Vec::new());
            return;
        };
        let generation = self.palette.generation;
        cx.spawn(async move |this, cx| {
            let mut files = Vec::new();
            let mut level = vec![String::new()];
            let mut visited = 0;
            while !level.is_empty() && visited < MAX_DIRS && files.len() < MAX_FILES {
                let batch: Vec<String> = level.drain(..).take(MAX_DIRS - visited).collect();
                visited += batch.len();
                let calls: Vec<_> = batch
                    .iter()
                    .map(|rel| {
                        daemon.call::<Vec<DirEntry>>(Call::ListDir {
                            path: crate::panes::tree::join(&project, rel),
                            sizes: false,
                            marks: false,
                        })
                    })
                    .collect();
                let mut next = Vec::new();
                for (rel, call) in batch.into_iter().zip(calls) {
                    for entry in call.await.unwrap_or_default() {
                        if entry.name.starts_with('.') && entry.kind == EntryKind::Dir {
                            continue;
                        }
                        let path = crate::panes::tree::join(&rel, &entry.name);
                        match entry.kind {
                            EntryKind::Dir if !SKIP.contains(&entry.name.as_str()) => {
                                next.push(path)
                            }
                            EntryKind::File => files.push(path),
                            _ => {}
                        }
                    }
                }
                level = next;
            }
            files.truncate(MAX_FILES);
            this.update(cx, |this, cx| {
                if this.palette.generation == generation {
                    this.palette.files = Some(files);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn palette_items(&self, cx: &App) -> Vec<Item> {
        let file_items = self.palette.files.iter().flatten().map(|rel| {
            let (detail, name) = match rel.rsplit_once('/') {
                Some((dir, name)) => (Some(dir.to_string()), name.to_string()),
                None => (None, rel.clone()),
            };
            Item {
                label: name,
                detail,
                choice: Choice::File(rel.clone()),
            }
        });
        let commands = self.commands().into_iter().map(|c| Item {
            label: c.label,
            detail: (!c.keys.is_empty()).then(|| look::keys(c.keys).to_string()),
            choice: Choice::Cmd(c.cmd),
        });
        let query = self.palette_input.read(cx).value().trim().to_string();
        if query.is_empty() {
            return file_items.take(8).chain(commands).collect();
        }
        let mut scored: Vec<(f32, Item)> = file_items
            .chain(commands)
            .filter_map(|item| {
                let text = match (&item.choice, &item.detail) {
                    (Choice::File(rel), _) => rel.clone(),
                    _ => item.label.clone(),
                };
                match_score(&text, &query).map(|s| (s, item))
            })
            .collect();
        scored.sort_by(|a, b| b.0.total_cmp(&a.0));
        scored.into_iter().take(SHOWN).map(|(_, i)| i).collect()
    }

    fn palette_move(&mut self, by: isize, cx: &mut Context<Self>) {
        let n = self.palette_items(cx).len() as isize;
        if n == 0 {
            return;
        }
        let ix = (self.palette.index as isize + by).rem_euclid(n) as usize;
        self.palette.index = ix;
        self.palette.scroll.scroll_to_item(ix);
        cx.notify();
    }

    fn palette_choose(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.palette_items(cx).into_iter().nth(ix) else {
            return;
        };
        self.close_palette(window, cx);
        match item.choice {
            Choice::File(rel) => self.open_file(rel, cx),
            Choice::Cmd(cmd) => self.run_command(cmd, window, cx),
        }
    }

    pub(crate) fn render_palette(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.palette.shown {
            return None;
        }
        let p = *Palette::get(cx);
        let items = self.palette_items(cx);
        let selected = p.accent.opacity(0.16);
        let rows: Vec<AnyElement> = items
            .iter()
            .enumerate()
            .map(|(ix, item)| {
                let leading: AnyElement = match &item.choice {
                    Choice::File(_) => picture(&look::file_icon(&item.label)).into_any_element(),
                    Choice::Cmd(_) => div()
                        .text_color(p.muted)
                        .child(icon("chevron.right"))
                        .into_any_element(),
                };
                h_flex()
                    .id(("pal", ix))
                    .flex_none()
                    .h(px(look::row()))
                    .px(px(look::GAP_8))
                    .gap(px(look::GAP_4))
                    .rounded(px(look::CONTROL_RADIUS))
                    .cursor_pointer()
                    .when(ix == self.palette.index, |d| d.bg(selected))
                    .child(div().w(px(look::ICON)).flex_none().child(leading))
                    .child(
                        div()
                            .flex_none()
                            .max_w(relative(0.6))
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(item.label.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_right()
                            .text_color(p.faint)
                            .children(item.detail.clone()),
                    )
                    .on_mouse_move(cx.listener(move |this, _, _, cx| {
                        if this.palette.index != ix {
                            this.palette.index = ix;
                            cx.notify();
                        }
                    }))
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.palette_choose(ix, window, cx)),
                    )
                    .into_any_element()
            })
            .collect();
        let empty = rows.is_empty();
        let card = v_flex()
            .id("palette-card")
            .occlude()
            .w(px(560.))
            .max_w(relative(0.9))
            .p(px(look::GAP_6))
            .gap(px(look::GAP_4))
            .rounded(px(look::RADIUS))
            .bg(p.panel)
            .border_1()
            .border_color(p.glass_edge)
            .shadow_lg()
            .text_size(px(look::small()))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                match event.keystroke.key.as_str() {
                    "up" => this.palette_move(-1, cx),
                    "down" => this.palette_move(1, cx),
                    "escape" => this.close_palette(window, cx),
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .child(
                h_flex()
                    .h(px(look::control_card()))
                    .px(px(look::GAP_8))
                    .gap(px(look::GAP_4))
                    .rounded(px(look::CONTROL_RADIUS))
                    .bg(p.ink(6.))
                    .text_color(p.faint)
                    .child(icon("search"))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_color(p.text)
                            .child(Input::new(&self.palette_input).appearance(false)),
                    )
                    .child(
                        div()
                            .px(px(look::GAP_4))
                            .rounded(px(look::CONTROL_RADIUS))
                            .border_1()
                            .border_color(p.glass_edge)
                            .text_size(px(look::small()))
                            .child("esc"),
                    ),
            )
            .child(
                v_flex()
                    .id("palette-list")
                    .max_h(px(340.))
                    .overflow_y_scroll()
                    .track_scroll(&self.palette.scroll)
                    .children(rows)
                    .when(empty, |d| {
                        d.child(div().px(px(10.)).py(px(6.)).text_color(p.muted).child(
                            if self.palette.files.is_none() {
                                "Looking through the project…"
                            } else {
                                "Nothing matches"
                            },
                        ))
                    }),
            );
        Some(
            deferred(
                div()
                    .id("palette-backdrop")
                    .absolute()
                    .inset_0()
                    .flex()
                    .justify_center()
                    .items_start()
                    .pt(px(70.))
                    .bg(p.shadow.opacity(0.12))
                    .child(card)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| this.close_palette(window, cx)),
                    ),
            )
            .with_priority(4)
            .into_any_element(),
        )
    }
}
