//! Sandhill drawn with GPUI: a native window in place of the Tauri app's web
//! view. It reaches sandhilld exactly as the Tauri app does, so kernels,
//! saved outputs and servers behave the same.
//!
//! Usage: `sandhill [--host HOST] [PROJECT_FOLDER]`, where HOST is
//! `local` (the default) or an ssh destination such as `lab`.

mod agents;
mod app_uninstall;
mod connection;
mod daemon;
mod editor;
mod explorer;
mod files;
mod folder_dialog;
mod git;
mod git_diff;
mod git_log;
mod git_pane;
mod git_rewrite;
mod help;
mod irkernel;
mod jobs;
mod kernel_menu;
mod look;
mod machine;
mod object_explorer;
mod packages;
mod plots;
mod prefs;
mod project_settings;
mod resource_manager;
mod shell;
mod start_screen;
mod terminal;
mod tree;
mod tree_actions;
mod trust;
mod view;
mod viewer;
mod workbench;

use std::sync::Arc;

use shell::{app_window, main_menu};

use gpui_kit::*;
use tracing_subscriber::EnvFilter;

actions!(
    sandhill,
    [
        Quit,
        Save,
        RunCell,
        RunLine,
        RunAll,
        Interrupt,
        ToggleConsole,
        Preview,
        SaveAll,
        CloseTab,
        CloseWindow,
        NewFile,
        NewFolder,
        OpenProject,
        ConnectServer,
        ToggleFiles,
        ToggleRight,
        ToggleAgents,
        TogglePalette,
        HelpAtCursor,
        About,
        HideSandhill,
        HideOthers,
        ShowAll,
        Minimize,
        Zoom,
        ToggleFullScreen,
        NewProject,
        NewWindow,
        AddCellAbove,
        AddCellBelow,
        DeleteCell,
        CellToCode,
        CellToMarkdown,
        SelectPrevCell,
        SelectNextCell,
        EditCell,
        UndoDeleteCell
    ]
);

/// The key context of a notebook's cells when none is being edited.
pub const CELLS: &str = "NotebookCells";

/// Register R and specialize imports and call argument names in R and Python.
fn register_r() {
    use gpui_kit::component::highlighter::{LanguageConfig, LanguageRegistry};
    let registry = LanguageRegistry::singleton();
    // gpui-component accepts only fixed capture names. Reserve `primary` for
    // Python import keywords, `hint` for R package-loading calls, and `label`
    // for call argument names. Their styles live in look::xcode_syntax.
    // Put specializations first: equal-range captures keep the first style.
    let r_highlights = format!(
        r#"
((call function: [
    (identifier) @hint
    (namespace_operator rhs: (identifier) @hint)
]) (#match? @hint "^(library|require)$"))
(call arguments: (arguments (argument name: (identifier) @label)))
{}
"#,
        tree_sitter_r::HIGHLIGHTS_QUERY
    );
    let config = LanguageConfig::new(
        "r",
        tree_sitter_r::LANGUAGE.into(),
        vec![],
        &r_highlights,
        "",
        tree_sitter_r::LOCALS_QUERY,
    );
    registry.register("r", &config);

    // Keep the built-in grammar, other captures, locals, and injections intact.
    let mut python = registry
        .language("python")
        .expect("the tree-sitter-python feature registers Python");
    python.highlights = format!(
        r#"
(import_statement "import" @primary)
(import_from_statement ["from" "import"] @primary)
(future_import_statement ["from" "import"] @primary)
(aliased_import "as" @primary)
(call arguments: (argument_list (keyword_argument name: (identifier) @label)))
; Xcode colours these reserved words (including the `in` of `for`) as keywords.
["and" "in" "is" "not" "or" "is not" "not in"] @keyword
{}
"#,
        python.highlights
    )
    .into();
    registry.register("python", &python);
}

/// R's typing rules: brackets, quotes and backtick names close themselves
/// outside strings and comments; Python keeps Kit's own rules, which do so too.
fn register_r_editing(cx: &mut App) {
    use gpui_kit::component::input::language_config::LanguageConfig;
    use gpui_kit::component::input::{AutoClosingPair, SyntaxContext, set_language_config};
    let pairs = [
        ("(", ")"),
        ("[", "]"),
        ("{", "}"),
        ("\"", "\""),
        ("'", "'"),
        ("`", "`"),
    ]
    .map(|(open, close)| {
        AutoClosingPair::new(open, close).not_in([SyntaxContext::String, SyntaxContext::Comment])
    });
    set_language_config("r", LanguageConfig::default().auto_closing_pairs(pairs), cx);
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
            terminal::init(cx);
            look::install(cx);
            cx.on_action(|_: &Quit, cx| app_window::quit(cx));
            // With no window open, a new one.
            // One main window: a new window is Welcome, to open a project.
            cx.on_action(|_: &NewWindow, cx| app_window::show_welcome(None, cx));
            register_r();
            register_r_editing(cx);
            main_menu::install_app_actions(cx);
            // The Tauri app's shortcuts: ⌘ on a Mac, Ctrl elsewhere.
            let m = if cfg!(target_os = "macos") {
                "cmd"
            } else {
                "ctrl"
            };
            // Bindings keep their keys for the life of the app.
            let k =
                |keys: &str| -> &'static str { Box::leak(keys.replace("mod", m).into_boxed_str()) };
            // Elsewhere Ctrl keys belong to the shell in the terminal.
            let g = if cfg!(target_os = "macos") {
                None
            } else {
                Some("!Terminal")
            };
            let mut keys = vec![
                KeyBinding::new(k("mod-q"), Quit, g),
                KeyBinding::new(k("mod-s"), Save, g),
                KeyBinding::new(k("alt-mod-s"), SaveAll, g),
                KeyBinding::new(k("mod-w"), CloseTab, g),
                KeyBinding::new(k("shift-mod-w"), CloseWindow, g),
                KeyBinding::new(k("mod-n"), NewFile, g),
                KeyBinding::new(k("alt-mod-n"), NewFolder, g),
                KeyBinding::new(k("mod-o"), OpenProject, g),
                KeyBinding::new(k("shift-mod-o"), ConnectServer, g),
                KeyBinding::new(k("shift-mod-n"), NewProject, g),
                // In a cell's editor, Shift+Enter runs the cell instead of
                // adding a line, as in Jupyter.
                KeyBinding::new("shift-enter", RunCell, Some("Input")),
                // ⌘↵ runs the line or statement in a script, and the cell
                // (staying on it) in a notebook.
                KeyBinding::new(k("mod-enter"), RunLine, Some("Input")),
                KeyBinding::new(k("alt-mod-r"), RunAll, g),
                KeyBinding::new(k("mod-shift-enter"), RunAll, g),
                KeyBinding::new(k("mod-."), Interrupt, g),
                KeyBinding::new(k("mod-b"), ToggleFiles, g),
                KeyBinding::new(k("alt-mod-b"), ToggleRight, g),
                KeyBinding::new(k("mod-i"), ToggleAgents, g),
                KeyBinding::new(k("mod-j"), ToggleConsole, g),
                KeyBinding::new(k("mod-k"), TogglePalette, g),
                KeyBinding::new(k("shift-mod-k"), Preview, g),
                KeyBinding::new("f1", HelpAtCursor, None),
                // A notebook's cells, when none is being edited (Esc
                // leaves a cell's editor), as in Jupyter.
                KeyBinding::new("a", AddCellAbove, Some(CELLS)),
                KeyBinding::new("b", AddCellBelow, Some(CELLS)),
                KeyBinding::new("d d", DeleteCell, Some(CELLS)),
                KeyBinding::new("z", UndoDeleteCell, Some(CELLS)),
                KeyBinding::new("y", CellToCode, Some(CELLS)),
                KeyBinding::new("m", CellToMarkdown, Some(CELLS)),
                KeyBinding::new("up", SelectPrevCell, Some(CELLS)),
                KeyBinding::new("k", SelectPrevCell, Some(CELLS)),
                KeyBinding::new("down", SelectNextCell, Some(CELLS)),
                KeyBinding::new("j", SelectNextCell, Some(CELLS)),
                KeyBinding::new("enter", EditCell, Some(CELLS)),
                KeyBinding::new("shift-enter", RunCell, Some(CELLS)),
                KeyBinding::new(k("mod-enter"), RunLine, Some(CELLS)),
            ];
            if cfg!(target_os = "macos") {
                keys.push(KeyBinding::new("cmd-m", Minimize, None));
                keys.push(KeyBinding::new("ctrl-cmd-f", ToggleFullScreen, None));
                keys.push(KeyBinding::new(
                    "ctrl-cmd-enter",
                    crate::shell::focus_mode::FocusEditor,
                    None,
                ));
            }
            cx.bind_keys(keys);
            cx.set_menus(main_menu::app_menus());
            // The project named on the command line opens in a window of
            // its own; otherwise the windows open when the app last quit.
            let places = match project.clone() {
                Some(path) => {
                    let place = app_window::Place {
                        host: host.clone(),
                        path,
                    };
                    vec![app_window::Places {
                        tabs: vec![place.clone()],
                        current: Some(place),
                    }]
                }
                None if host != sandhill_link::LOCAL => {
                    let place = app_window::Place {
                        host: host.clone(),
                        path: String::new(),
                    };
                    vec![app_window::Places {
                        tabs: vec![place.clone()],
                        current: Some(place),
                    }]
                }
                None => prefs::windows(),
            };
            // Completion menus and their documentation stay clear of the
            // title and status bars.
            gpui_kit::component::input::set_popup_insets(px(look::TITLE_BAR), px(look::STATUS_BAR));
            app_window::start(rt.clone(), places, cx);
            cx.activate(true);
        });
    Ok(())
}
