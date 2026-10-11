//! The macOS menu bar. Commands that act on documents are sent to the UI as
//! `menu://command` events with the item's id.

use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Emitter, Manager, Runtime};

pub const COMMAND: &str = "menu://command";

/// Ids the UI handles (`app/src/ui/App.tsx`).
const SAVE: &str = "save";
const CLOSE_TAB: &str = "close-tab";
const RUN_ALL: &str = "run-all";
const QUIT: &str = "quit";

pub fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let separator = || PredefinedMenuItem::separator(app);
    let sandhill = Submenu::with_items(
        app,
        "Sandhill",
        true,
        &[
            &PredefinedMenuItem::about(app, Some("About Sandhill"), None)?,
            &separator()?,
            &PredefinedMenuItem::services(app, None)?,
            &separator()?,
            &PredefinedMenuItem::hide(app, None)?,
            &PredefinedMenuItem::hide_others(app, None)?,
            &PredefinedMenuItem::show_all(app, None)?,
            &separator()?,
            // Not the predefined Quit: closing the window first lets the UI
            // ask about unsaved changes.
            &MenuItem::with_id(app, QUIT, "Quit Sandhill", true, Some("CmdOrCtrl+Q"))?,
        ],
    )?;
    let file = Submenu::with_items(
        app,
        "File",
        true,
        &[
            &MenuItem::with_id(app, SAVE, "Save", true, Some("CmdOrCtrl+S"))?,
            &separator()?,
            // Not the predefined Close Window, which would take Cmd+W.
            &MenuItem::with_id(app, CLOSE_TAB, "Close Tab", true, Some("CmdOrCtrl+W"))?,
        ],
    )?;
    // Undo and Redo are left out so Cmd+Z reaches the editor, which keeps
    // its own history. The clipboard items are needed for Cmd+C/V to work
    // in the web view at all.
    let edit = Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;
    let run = Submenu::with_items(
        app,
        "Run",
        true,
        &[&MenuItem::with_id(
            app,
            RUN_ALL,
            "Run All",
            true,
            Some("CmdOrCtrl+Alt+R"),
        )?],
    )?;
    let window = Submenu::with_items(
        app,
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(app, None)?,
            &PredefinedMenuItem::maximize(app, Some("Zoom"))?,
            &PredefinedMenuItem::fullscreen(app, None)?,
        ],
    )?;
    Menu::with_items(app, &[&sandhill, &file, &edit, &run, &window])
}

pub fn handle<R: Runtime>(app: &AppHandle<R>, event: MenuEvent) {
    match event.id().as_ref() {
        QUIT => match app.get_webview_window("main") {
            // The UI confirms, then the app exits with its last window.
            Some(window) => {
                let _ = window.close();
            }
            None => app.exit(0),
        },
        id @ (SAVE | CLOSE_TAB | RUN_ALL) => {
            let _ = app.emit(COMMAND, id);
        }
        _ => {}
    }
}
