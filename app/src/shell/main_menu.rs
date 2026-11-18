//! The menus: the Mac menu bar, as the Tauri app's `menu.rs` builds it,
//! and the ☰ menu in the title bar (other projects and servers, project
//! settings, showing the console, and where notebooks' output goes), as
//! its `MainMenu` has it.

use gpui_kit::component::input;
use gpui_kit::*;

use crate::look::{self, Palette};
use crate::panes::tree_actions::{menu_item, separator};
use crate::shell::command_palette::Cmd;
use crate::workbench::Workbench;
use crate::{
    About, CloseTab, CloseWindow, ConnectServer, HideOthers, HideSandhill, Minimize, NewFile,
    NewFolder, NewProject, NewWindow, OpenProject, OpenSettings, Quit, RunAll, Save, SaveAll,
    ShowAll, ToggleAgents, ToggleConsole, ToggleFiles, ToggleFullScreen, TogglePalette,
    ToggleRight, Zoom,
};

/// The menu bar. Shortcuts show beside the items from the key bindings.
pub fn app_menus() -> Vec<Menu> {
    vec![
        Menu::new("Sandhill").items([
            MenuItem::action("About Sandhill", About),
            MenuItem::separator(),
            MenuItem::action("Settings…", OpenSettings),
            MenuItem::separator(),
            MenuItem::os_submenu("Services", SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action("Hide Sandhill", HideSandhill),
            MenuItem::action("Hide Others", HideOthers),
            MenuItem::action("Show All", ShowAll),
            MenuItem::separator(),
            MenuItem::action("Quit Sandhill", Quit),
        ]),
        Menu::new("File").items([
            MenuItem::action("New File…", NewFile),
            MenuItem::action("New Folder…", NewFolder),
            MenuItem::action("New Project…", NewProject),
            MenuItem::separator(),
            MenuItem::action("Open Project…", OpenProject),
            MenuItem::action("Connect to Server…", ConnectServer),
            MenuItem::separator(),
            MenuItem::action("Save", Save),
            MenuItem::action("Save All", SaveAll),
            MenuItem::separator(),
            MenuItem::action("Close Tab", CloseTab),
            MenuItem::action("Close Window", CloseWindow),
        ]),
        Menu::new("Edit").items([
            MenuItem::os_action("Undo", input::Undo, OsAction::Undo),
            MenuItem::os_action("Redo", input::Redo, OsAction::Redo),
            MenuItem::separator(),
            MenuItem::os_action("Cut", input::Cut, OsAction::Cut),
            MenuItem::os_action("Copy", input::Copy, OsAction::Copy),
            MenuItem::os_action("Paste", input::Paste, OsAction::Paste),
            MenuItem::os_action("Select All", input::SelectAll, OsAction::SelectAll),
        ]),
        Menu::new("View").items([
            MenuItem::action("Go to File or Command…", TogglePalette),
            MenuItem::separator(),
            MenuItem::action("Left Panel", ToggleFiles),
            MenuItem::action("AI Panel", ToggleAgents),
            MenuItem::action("Console", ToggleConsole),
            MenuItem::action("Focus Editor", crate::shell::focus_mode::FocusEditor),
        ]),
        Menu::new("Run").items([MenuItem::action("Run All", RunAll)]),
        Menu::new("Window").items([
            MenuItem::action("Minimize", Minimize),
            MenuItem::action("Zoom", Zoom),
            MenuItem::action("Enter Full Screen", ToggleFullScreen),
            MenuItem::separator(),
            MenuItem::action("New Window", NewWindow),
        ]),
    ]
}

/// What the app does for the menu bar's own items, with or without a window.
pub fn install_app_actions(cx: &mut App) {
    cx.on_action(|_: &HideSandhill, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
}

impl Workbench {
    /// The menu bar's items for the project, on the root element; the
    /// window's own (closing it, zooming) are the window's.
    pub(crate) fn window_actions(&self, d: Div, cx: &mut Context<Self>) -> Div {
        d.on_action(cx.listener(|this, _: &SaveAll, w, cx| this.run_command(Cmd::SaveAll, w, cx)))
            .on_action(
                cx.listener(|this, _: &CloseTab, w, cx| this.run_command(Cmd::CloseTab, w, cx)),
            )
            .on_action(cx.listener(|this, _: &NewFile, w, cx| {
                this.run_command(Cmd::New(crate::panes::files::NewKind::File), w, cx)
            }))
            .on_action(cx.listener(|this, _: &NewFolder, w, cx| {
                this.run_command(Cmd::New(crate::panes::files::NewKind::Folder), w, cx)
            }))
            .on_action(
                cx.listener(|this, _: &OpenProject, w, cx| {
                    this.run_command(Cmd::OpenProject, w, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &ConnectServer, w, cx| this.run_command(Cmd::Connect, w, cx)),
            )
            .on_action(
                cx.listener(|this, _: &ToggleFiles, w, cx| {
                    this.run_command(Cmd::ToggleFiles, w, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &ToggleAgents, w, cx| {
                this.run_command(Cmd::ToggleAgents, w, cx)
            }))
            .on_action(
                cx.listener(|this, _: &ToggleRight, w, cx| {
                    this.run_command(Cmd::ToggleRight, w, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &crate::shell::focus_mode::FocusEditor, w, cx| {
                    this.run_command(Cmd::FocusEditor, w, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &TogglePalette, w, cx| this.toggle_palette(w, cx)))
            .on_action(
                cx.listener(|this, _: &crate::HelpAtCursor, w, cx| this.help_at_cursor(w, cx)),
            )
            .on_action(
                cx.listener(|this, _: &NewProject, w, cx| {
                    this.run_command(Cmd::OpenProject, w, cx)
                }),
            )
    }

    /// The ☰ menu, under its button in the title bar.
    pub(crate) fn render_main_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.main_menu {
            return None;
        }
        let p = *Palette::get(cx);
        let check = |on: bool| on.then_some("check");
        let mut rows = vec![
            menu_item(
                "mm-connect",
                Some("device.cloud"),
                "Connect to server…",
                "⇧⌘O",
                &p,
                cx,
                |this, w, cx| this.run_command(Cmd::Connect, w, cx),
            ),
            menu_item(
                "mm-open",
                Some("folder"),
                "Open project…",
                "⌘O",
                &p,
                cx,
                |this, w, cx| this.run_command(Cmd::OpenProject, w, cx),
            ),
        ];
        let recent: Vec<_> = self
            .recent
            .iter()
            .filter(|r| r.exists && Some(&r.path) != self.project.as_ref())
            .take(5)
            .cloned()
            .collect();
        if !recent.is_empty() {
            let host = crate::welcome::start_screen::host_label(&self.host());
            rows.push(separator(&p));
            rows.push(
                div()
                    .px(px(8.))
                    .py(px(4.))
                    .text_size(px(look::small()))
                    .text_color(p.faint)
                    .child(format!("Recent on {host}"))
                    .into_any_element(),
            );
            for (i, r) in recent.into_iter().enumerate() {
                let path = r.path.clone();
                rows.push(menu_item(
                    ("mm-recent", i),
                    Some("folder"),
                    &r.name,
                    "",
                    &p,
                    cx,
                    move |this, window, cx| {
                        let host = this.host();
                        let path = path.clone();
                        this.with_shell(window, cx, move |shell, window, cx| {
                            shell.open_place(host, path, window, cx);
                        });
                    },
                ));
            }
        }
        if self.project.is_some() {
            rows.push(separator(&p));
            rows.push(menu_item(
                "mm-settings",
                Some("settings"),
                "Project settings…",
                "",
                &p,
                cx,
                |this, w, cx| this.run_command(Cmd::ProjectSettings, w, cx),
            ));
            rows.push(menu_item(
                "mm-close",
                Some("close"),
                "Close project",
                "",
                &p,
                cx,
                |this, w, cx| this.run_command(Cmd::CloseProject, w, cx),
            ));
        }
        rows.push(separator(&p));
        rows.push(menu_item(
            "mm-middle",
            check(self.show_middle),
            "Show editor",
            "",
            &p,
            cx,
            |this, w, cx| this.run_command(Cmd::ToggleMiddle, w, cx),
        ));
        rows.push(menu_item(
            "mm-console",
            check(self.show_console),
            "Show console",
            "⌘J",
            &p,
            cx,
            |this, w, cx| this.run_command(Cmd::ToggleConsole, w, cx),
        ));
        rows.push(menu_item(
            "mm-focus-editor",
            check(self.focus_mode.active()),
            "Focus Editor",
            "⌃⌘↩",
            &p,
            cx,
            |this, w, cx| this.run_command(Cmd::FocusEditor, w, cx),
        ));
        rows.push(separator(&p));
        rows.push(
            div()
                .px(px(8.))
                .py(px(4.))
                .text_size(px(look::small()))
                .text_color(p.faint)
                .child("Notebooks and Quarto")
                .into_any_element(),
        );
        rows.push(menu_item(
            "mm-nb-console",
            check(self.notebook_console),
            "Show their runs in the Console",
            "",
            &p,
            cx,
            |this, w, cx| this.run_command(Cmd::NotebookConsole, w, cx),
        ));
        rows.push(menu_item(
            "mm-nb-plots",
            check(self.notebook_plots),
            "Show their plots in Plots",
            "",
            &p,
            cx,
            |this, w, cx| this.run_command(Cmd::NotebookPlots, w, cx),
        ));
        rows.push(menu_item(
            "mm-console-plots",
            check(self.plot_history.console_auto),
            "Send Console plots automatically",
            "",
            &p,
            cx,
            |this, _, cx| this.toggle_console_plots(cx),
        ));
        let body = crate::agents::composer_view::popup(&p)
            .id("main-menu")
            .occlude()
            .w(px(280.))
            .children(rows)
            .on_mouse_down_out(cx.listener(|this, event: &MouseDownEvent, _, cx| {
                if this.popup_triggers.contains("main-menu", event.position) {
                    return;
                }
                this.main_menu = false;
                cx.notify();
            }));
        Some(
            div()
                .absolute()
                .top(px(look::TITLE_BAR + 2.))
                .left(px(crate::view::TITLE_BAR_LEFT + 104.))
                .child(deferred(body).with_priority(3))
                .into_any_element(),
        )
    }
}
