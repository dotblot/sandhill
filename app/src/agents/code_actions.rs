//! What the user can do with code in the AI pane, in replies' code blocks
//! and in the cards of the code an agent ran (Huan, 2026-11-16): Copy, and
//! under ⋯ Insert at Cursor, Add Cell Below, Run in Console and New File.

use gpui_kit::component::h_flex;
use gpui_kit::*;
use sandhill_proto::{Call, FileWritten};

use crate::agents::Agents;
use crate::editor::notebook::CellKind;
use crate::look::{self, Palette, icon_button};
use crate::workbench::Workbench;

/// The ⋯ menu of a piece of code, where it opened.
pub(crate) struct CodeMenu {
    pub at: Point<Pixels>,
    pub code: String,
    pub language: Option<String>,
}

/// Copy and ⋯ for `code`; `id` tells blocks apart.
pub(crate) fn code_buttons(
    id: impl Into<ElementId>,
    code: SharedString,
    language: Option<SharedString>,
    agents: WeakEntity<Agents>,
    p: &Palette,
) -> Div {
    let id = id.into();
    let copied = code.clone();
    let tip = |text: &'static str| {
        move |w: &mut Window, cx: &mut App| {
            gpui_kit::component::tooltip::Tooltip::new(text).build(w, cx)
        }
    };
    h_flex()
        .gap(px(2.))
        .child(
            icon_button((id.clone(), "copy"), "edit.copy", p, false)
                .tooltip(tip("Copy"))
                .on_click(move |_, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(copied.to_string()))
                }),
        )
        .child(
            icon_button((id, "more"), "more", p, false)
                .tooltip(tip("Insert, add as a cell, run…"))
                .on_mouse_down(MouseButton::Left, move |event, _, cx| {
                    let menu = CodeMenu {
                        at: event.position,
                        code: code.to_string(),
                        language: language.as_ref().map(|l| l.to_lowercase()),
                    };
                    agents
                        .update(cx, |agents, cx| {
                            agents.code_menu = Some(menu);
                            cx.notify();
                        })
                        .ok();
                    cx.stop_propagation();
                }),
        )
}

/// What can be done with code now: the active document takes text at its
/// cursor, a notebook or Quarto document takes a cell, the Console runs it,
/// and the project takes a new file.
struct Can {
    insert: bool,
    cell: bool,
    console: bool,
    file: bool,
}

impl Agents {
    /// The code's ⋯ menu.
    pub(crate) fn render_code_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        use crate::shell::text_menu::{render, row};
        let menu = self.code_menu.as_ref()?;
        let p = *Palette::get(cx);
        let can = self
            .workbench
            .read_with(cx, |w, cx| Can {
                insert: w.can_insert_into_active_document(cx),
                cell: w.can_add_cell_below(),
                console: w.variables_session().is_some(),
                file: w.project.is_some(),
            })
            .unwrap_or(Can {
                insert: false,
                cell: false,
                console: false,
                file: false,
            });
        let mut rows = Vec::new();
        let mut add = |id: &'static str,
                       label: &'static str,
                       why: &'static str,
                       on: bool,
                       run: fn(
            &mut Workbench,
            String,
            Option<String>,
            &mut Window,
            &mut Context<Workbench>,
        ),
                       cx: &mut Context<Self>| {
            rows.push(if on {
                row(id, label, "", &p, cx, move |this: &mut Self, window, cx| {
                    if let Some(menu) = this.code_menu.take() {
                        this.workbench
                            .update(cx, |w, cx| run(w, menu.code, menu.language, window, cx))
                            .ok();
                    }
                })
            } else {
                disabled_row(label, why, &p)
            });
        };
        add(
            "code-insert",
            "Insert at Cursor",
            "Open a script or a code cell first",
            can.insert,
            |w, code, _, window, cx| {
                w.insert_into_active_document(&code, window, cx);
            },
            cx,
        );
        add(
            "code-cell",
            "Add Cell Below",
            "Open a notebook or Quarto document first",
            can.cell,
            |w, code, language, window, cx| w.add_cell_below(code, language, window, cx),
            cx,
        );
        add(
            "code-console",
            "Run in Console",
            "Start a kernel in the Console first",
            can.console,
            |w, code, _, _, cx| w.run_in_console(code, cx),
            cx,
        );
        add(
            "code-file",
            "New File",
            "Open a project first",
            can.file,
            |w, code, language, _, cx| w.new_file_with(code, language.as_deref(), cx),
            cx,
        );
        Some(render(
            "code-menu",
            menu.at,
            rows,
            &p,
            cx,
            |this: &mut Self| {
                this.code_menu = None;
            },
        ))
    }
}

/// A row that can't be used now, saying why.
fn disabled_row(label: &'static str, why: &'static str, p: &Palette) -> AnyElement {
    gpui_kit::component::v_flex()
        .px(px(8.))
        .py(px(3.))
        .child(div().text_color(p.faint).child(label))
        .child(
            div()
                .text_size(px(look::SMALL - 1.))
                .text_color(p.faint)
                .child(why),
        )
        .into_any_element()
}

impl Workbench {
    /// Whether the active document is a notebook or Quarto/R Markdown
    /// document that takes a new cell.
    pub(crate) fn can_add_cell_below(&self) -> bool {
        self.open
            .as_ref()
            .is_some_and(|o| !o.single_editor() && o.notebook.can_add_cells())
    }

    /// Adds `code` as a code cell below the active document's current cell
    /// (after the last, without one); in Quarto, a chunk of the code's
    /// `language` when it names R or Python.
    pub(crate) fn add_cell_below(
        &mut self,
        code: String,
        language: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(open) = self.open.as_ref() else {
            return;
        };
        let at = open
            .current
            .map_or(open.notebook.cells.len(), |current| current + 1);
        let language = match language.as_deref() {
            Some("r") => Some("r"),
            Some("python" | "py" | "python3") => Some("python"),
            _ => None,
        };
        self.add_cell_in(at, CellKind::Code, code, language, window, cx);
    }

    /// Saves `code` as a new file in the project's top folder,
    /// `untitled.py` (`untitled-2.py`… when taken), and opens it.
    pub(crate) fn new_file_with(
        &mut self,
        code: String,
        language: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let (Some(daemon), Some(project)) = (self.daemon(), self.project.clone()) else {
            return;
        };
        let extension = extension_for(language.or(self.active_language().as_deref()));
        cx.spawn(async move |this, cx| {
            let mut last = None;
            for n in 1..=50 {
                let name = if n == 1 {
                    format!("untitled.{extension}")
                } else {
                    format!("untitled-{n}.{extension}")
                };
                let written = daemon
                    .call::<FileWritten>(Call::WriteFile {
                        path: crate::panes::files::join_path(&project, &name),
                        text: code.clone(),
                        expected_mtime_ms: None,
                        base64: false,
                        create: true,
                    })
                    .await;
                match written {
                    Ok(_) => {
                        this.update(cx, |this, cx| {
                            this.refresh_files(cx);
                            this.open_file(name, cx);
                        })
                        .ok();
                        return;
                    }
                    Err(err) => last = Some(err),
                }
            }
            if let Some(err) = last {
                this.update(cx, |this, cx| {
                    this.problem = Some(format!("Could not make a new file: {err:#}"));
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }
}

/// A file extension for code in `language` (a fence's or the session's).
fn extension_for(language: Option<&str>) -> &'static str {
    match language.map(str::to_lowercase).as_deref() {
        Some("r") => "R",
        Some("python" | "py" | "python3") => "py",
        Some("bash" | "sh" | "shell" | "zsh" | "console") => "sh",
        Some("sql") => "sql",
        Some("markdown" | "md") => "md",
        Some("quarto" | "qmd") => "qmd",
        Some("yaml" | "yml") => "yaml",
        Some("json") => "json",
        Some("nextflow" | "nf") => "nf",
        Some("julia" | "jl") => "jl",
        _ => "txt",
    }
}
