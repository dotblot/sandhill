//! Sandhill's Mac app: a window for the TypeScript UI in `app/src`, plus the
//! connection to sandhilld that the UI talks through.

mod daemon;
mod link;
mod memory;
mod menu;

use std::sync::Arc;

fn main() -> anyhow::Result<()> {
    memory::configure();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    let link = Arc::new(link::Link::default());
    tauri::Builder::default()
        .manage(link.clone())
        .menu(menu::build)
        .on_menu_event(menu::handle)
        .invoke_handler(tauri::generate_handler![
            link::daemon_call,
            link::link_state
        ])
        .register_asynchronous_uri_scheme_protocol(link::BLOB_SCHEME, link::blob_protocol)
        .setup(move |app| {
            link::start(app.handle().clone(), link);
            Ok(())
        })
        .run(tauri::generate_context!())?;
    Ok(())
}
