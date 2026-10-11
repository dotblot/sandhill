//! The macOS menu bar. Commands that act on documents are sent to the
//! focused window as `menu://command` events with the item's id.

use std::sync::Arc;

use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Emitter, EventTarget, Manager, Runtime, WebviewWindow};

use crate::windows::Windows;

pub const COMMAND: &str = "menu://command";

/// Ids the UI handles (`app/src/ui/App.tsx`).
const SAVE: &str = "save";
const CLOSE_TAB: &str = "close-tab";
const RUN_ALL: &str = "run-all";
const QUIT: &str = "quit";
const OPEN_PROJECT: &str = "open-project";
const CLOSE_WINDOW: &str = "close-window";

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
            &MenuItem::with_id(
                app,
                OPEN_PROJECT,
                "Open Project…",
                true,
                Some("CmdOrCtrl+O"),
            )?,
            &separator()?,
            &MenuItem::with_id(app, SAVE, "Save", true, Some("CmdOrCtrl+S"))?,
            &separator()?,
            // Not the predefined Close Window, which would take Cmd+W.
            &MenuItem::with_id(app, CLOSE_TAB, "Close Tab", true, Some("CmdOrCtrl+W"))?,
            &MenuItem::with_id(
                app,
                CLOSE_WINDOW,
                "Close Window",
                true,
                Some("CmdOrCtrl+Shift+W"),
            )?,
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
    let windows = app.state::<Arc<Windows>>();
    match event.id().as_ref() {
        // Each window confirms unsaved changes; the app exits with the last.
        QUIT => windows.quit(app),
        OPEN_PROJECT => {
            if let Err(err) = windows.open_start(app) {
                tracing::warn!("cannot open the start window: {err}");
            }
        }
        CLOSE_WINDOW => {
            if let Some(window) = focused(app) {
                let _ = window.close();
            }
        }
        id @ (SAVE | CLOSE_TAB | RUN_ALL) => {
            if let Some(window) = focused(app) {
                let target = EventTarget::webview_window(window.label());
                let _ = app.emit_to(target, COMMAND, id);
            }
        }
        _ => {}
    }
}

fn focused<R: Runtime>(app: &AppHandle<R>) -> Option<WebviewWindow<R>> {
    app.webview_windows()
        .into_values()
        .find(|w| w.is_focused().unwrap_or(false))
}
