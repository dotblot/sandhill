//! Sandhill's Mac app: windows for the TypeScript UI in `app/src`, plus the
//! connections to sandhilld that the UI talks through.

mod daemon;
mod link;
mod memory;
mod menu;
mod windows;

use std::sync::Arc;

use anyhow::Context;
use tauri::{Manager, RunEvent, WindowEvent};

use crate::link::Links;
use crate::windows::Windows;

fn main() -> anyhow::Result<()> {
    memory::configure();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    let app = tauri::Builder::default()
        .manage(Arc::new(Links::default()))
        .menu(menu::build)
        .on_menu_event(menu::handle)
        .invoke_handler(tauri::generate_handler![
            link::use_host,
            link::daemon_call,
            windows::window_context,
            windows::open_project_window,
            windows::show_start_window,
            windows::cancel_quit,
        ])
        .register_asynchronous_uri_scheme_protocol(link::BLOB_SCHEME, link::blob_protocol)
        .on_window_event(|window, event| {
            if let WindowEvent::Destroyed = event {
                let app = window.app_handle();
                app.state::<Arc<Windows>>().closed(window.label());
                app.state::<Arc<Links>>().release_window(window.label());
            }
        })
        .setup(|app| {
            let saved = app.path().app_data_dir()?.join("open-projects.json");
            let windows = Arc::new(Windows::new(saved));
            app.manage(windows.clone());
            windows.restore(app.handle())?;
            Ok(())
        })
        .build(tauri::generate_context!())
        .context("cannot start Sandhill")?;

    app.run(|app, event| {
        // Every window has closed. Unless the user quit, closing the last
        // project shows the start screen again.
        if let RunEvent::ExitRequested {
            code: None, api, ..
        } = event
        {
            let windows = app.state::<Arc<Windows>>();
            if windows.keep_running() {
                api.prevent_exit();
                if let Err(err) = windows.open_start(app) {
                    tracing::warn!("cannot open the start window: {err}");
                }
            }
        }
    });
    Ok(())
}
