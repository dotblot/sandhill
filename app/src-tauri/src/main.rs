//! Sandhill's Mac app: windows for the TypeScript UI in `app/src`, plus the
//! connections to sandhilld that the UI talks through.

mod link;
mod memory;
mod menu;
mod servers;
mod windows;

use std::sync::Arc;

use anyhow::Context;
use tauri::{Manager, RunEvent, WindowEvent};

use crate::link::Links;
use crate::servers::Servers;
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
        .menu(menu::build)
        .on_menu_event(menu::handle)
        .invoke_handler(tauri::generate_handler![
            link::use_host,
            link::daemon_call,
            link::reconnect_host,
            link::disconnect_host,
            windows::window_context,
            windows::set_window_places,
            windows::cancel_quit,
            windows::open_link,
            servers::list_servers,
            servers::add_server,
            servers::remove_server,
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
            app.manage(Arc::new(Links::new(link_options(app.handle()))));
            let data = app.path().app_data_dir()?;
            app.manage(Arc::new(Servers::load(data.join("servers.json"))));
            let saved = data.join("open-projects.json");
            let windows = Arc::new(Windows::new(saved));
            app.manage(windows.clone());
            windows.restore(app.handle())?;
            Ok(())
        })
        .build(tauri::generate_context!())
        .context("cannot start Sandhill")?;

    app.run(|app, event| {
        // Every window has closed. Unless the user quit, closing the last
        // window with projects shows the Start screen again.
        if let RunEvent::ExitRequested {
            code: None, api, ..
        } = event
        {
            let windows = app.state::<Arc<Windows>>();
            if windows.keep_running() {
                api.prevent_exit();
                if let Err(err) = windows.open_window(app, Default::default()) {
                    tracing::warn!("cannot open a window: {err}");
                }
            }
        }
    });
    Ok(())
}

/// The builds of sandhilld the app installs on servers: `daemons/` in the
/// app's resources (bundled by `npm run bundle`), or `$SANDHILL_DAEMON_BUILDS`.
/// `$SANDHILL_SSH` replaces the ssh program.
fn link_options<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> sandhill_link::Options {
    let builds = std::env::var_os("SANDHILL_DAEMON_BUILDS")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            let dir = app.path().resource_dir().ok()?.join("daemons");
            dir.is_dir().then_some(dir)
        });
    match &builds {
        Some(dir) => tracing::info!("builds of sandhilld for servers: {}", dir.display()),
        None => tracing::info!("no builds of sandhilld for servers in this app"),
    }
    sandhill_link::Options {
        builds,
        ssh: std::env::var_os("SANDHILL_SSH").map(Into::into),
    }
}
