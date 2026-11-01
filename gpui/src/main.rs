//! Sandhill drawn with GPUI: a trial of a native window in place of the web
//! view. It reaches sandhilld exactly as the Tauri app does, so kernels,
//! saved outputs and servers behave the same.
//!
//! Usage: `sandhill-gpui [PROJECT_FOLDER]`

mod daemon;
mod notebook;
mod shown;
mod workbench;

use std::sync::Arc;

use gpui_kit::*;
use tracing_subscriber::EnvFilter;

use crate::workbench::Workbench;

actions!(sandhill, [Quit, Save, RunCell, RunAll]);

/// Highlighting for R, with Posit's tree-sitter grammar (MIT).
fn register_r() {
    use gpui_kit::component::highlighter::{LanguageConfig, LanguageRegistry};
    let config = LanguageConfig::new(
        "r",
        tree_sitter_r::LANGUAGE.into(),
        vec![],
        tree_sitter_r::HIGHLIGHTS_QUERY,
        "",
        tree_sitter_r::LOCALS_QUERY,
    );
    LanguageRegistry::singleton().register("r", &config);
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();
    let project = std::env::args().nth(1).map(|p| {
        std::fs::canonicalize(&p)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or(p)
    });
    let rt = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()?,
    );

    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            cx.on_action(|_: &Quit, cx| cx.quit());
            register_r();
            let (save, run_all) = if cfg!(target_os = "macos") {
                ("cmd-s", "cmd-shift-enter")
            } else {
                ("ctrl-s", "ctrl-shift-enter")
            };
            cx.bind_keys([
                KeyBinding::new("cmd-q", Quit, None),
                KeyBinding::new(save, Save, None),
                // In a cell's editor, Shift+Enter runs the cell instead of
                // adding a line, as in Jupyter.
                KeyBinding::new("shift-enter", RunCell, Some("Input")),
                KeyBinding::new(run_all, RunAll, None),
            ]);
            cx.set_menus(vec![Menu {
                name: "Sandhill".into(),
                items: vec![MenuItem::action("Quit Sandhill", Quit)],
                disabled: false,
            }]);
            cx.on_window_closed(|cx, _| cx.quit()).detach();

            let bounds = Bounds::centered(None, size(px(1200.), px(800.)), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Sandhill (GPUI trial)".into()),
                    ..Default::default()
                }),
                ..Default::default()
            };
            gpui_kit::open_window(options, cx, |_, cx| {
                cx.new(|cx| Workbench::new(rt.clone(), project.clone(), cx))
            })
            .expect("could not open the window");
            cx.activate(true);
        });
    Ok(())
}
