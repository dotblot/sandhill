//! Sandhill drawn with GPUI: a native window in place of the Tauri app's web
//! view. It reaches sandhilld exactly as the Tauri app does, so kernels,
//! saved outputs and servers behave the same.
//!
//! Usage: `sandhill-gpui [--host HOST] [PROJECT_FOLDER]`, where HOST is
//! `local` (the default) or an ssh destination such as `lab`.

// The theme in `look` is one large `json!`.
#![recursion_limit = "512"]

mod agent_edits;
mod agents;
mod agents_box;
mod agents_view;
mod chat;
mod composer;
mod composer_view;
mod daemon;
mod explorer;
mod look;
mod notebook;
mod pages;
mod panes;
mod plots;
mod prefs;
mod shown;
mod statements;
mod terminal;
mod tree;
mod view;
mod workbench;

use std::sync::Arc;

use gpui_kit::*;
use tracing_subscriber::EnvFilter;

use crate::workbench::Workbench;

actions!(
    sandhill,
    [
        Quit,
        Save,
        RunCell,
        RunLine,
        RunAll,
        Interrupt,
        ToggleConsole
    ]
);

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
    let mut args = std::env::args().skip(1);
    let mut host = sandhill_link::LOCAL.to_string();
    let mut project = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--host" => host = args.next().unwrap_or(host),
            _ => project = Some(arg),
        }
    }
    let target = sandhill_link::Target::parse(&host)?;
    // A folder on this machine can be given relative to here.
    let project = project.map(|p| match target {
        sandhill_link::Target::Local => std::fs::canonicalize(&p)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or(p),
        _ => p,
    });
    let rt = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()?,
    );

    gpui_kit::application()
        .with_assets(look::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            look::install(cx);
            cx.on_action(|_: &Quit, cx| cx.quit());
            register_r();
            let (save, run_line, run_all, interrupt, console) = if cfg!(target_os = "macos") {
                ("cmd-s", "cmd-enter", "cmd-shift-enter", "cmd-.", "cmd-j")
            } else {
                (
                    "ctrl-s",
                    "ctrl-enter",
                    "ctrl-shift-enter",
                    "ctrl-.",
                    "ctrl-j",
                )
            };
            cx.bind_keys([
                KeyBinding::new("cmd-q", Quit, None),
                KeyBinding::new(save, Save, None),
                // In a cell's editor, Shift+Enter runs the cell instead of
                // adding a line, as in Jupyter.
                KeyBinding::new("shift-enter", RunCell, Some("Input")),
                // ⌘↵ runs the line or statement in a script, and the cell
                // (staying on it) in a notebook.
                KeyBinding::new(run_line, RunLine, Some("Input")),
                KeyBinding::new(run_all, RunAll, None),
                KeyBinding::new(interrupt, Interrupt, None),
                KeyBinding::new(console, ToggleConsole, None),
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
                // On a Mac the title bar is Sandhill's own row of capsules,
                // with the window buttons at its left, as in the Tauri app.
                titlebar: Some(TitlebarOptions {
                    title: Some("Sandhill".into()),
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(12.), px(10.))),
                }),
                ..Default::default()
            };
            gpui_kit::open_window(options, cx, |window, cx| {
                look::set_appearance(window.appearance(), cx);
                window
                    .observe_window_appearance(|window, cx| {
                        look::set_appearance(window.appearance(), cx)
                    })
                    .detach();
                cx.new(|cx| Workbench::new(rt.clone(), target.clone(), project.clone(), window, cx))
            })
            .expect("could not open the window");
            cx.activate(true);
        });
    Ok(())
}
