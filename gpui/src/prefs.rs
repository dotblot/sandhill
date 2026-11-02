//! The app's own settings on this Mac (or Linux machine): what the Tauri
//! app kept in the web view's local storage, such as starred models. A
//! small JSON file; reading or writing it failing is never an error.

use std::path::PathBuf;

use serde_json::{Value, json};

fn file() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let dir = if cfg!(target_os = "macos") {
        home.join("Library/Application Support/Sandhill")
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"))
            .join("sandhill")
    };
    Some(dir.join("gpui.json"))
}

fn read() -> Value {
    file()
        .and_then(|f| std::fs::read_to_string(f).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_else(|| json!({}))
}

fn write(key: &str, value: Value) {
    let Some(path) = file() else { return };
    let mut all = read();
    if let Some(map) = all.as_object_mut() {
        map.insert(key.to_string(), value);
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, all.to_string());
}

/// The models starred in the model menu, as `kind:id`.
pub fn favourites() -> Vec<String> {
    match read()["favouriteModels"].as_array() {
        Some(list) => list
            .iter()
            .filter_map(|k| k.as_str().map(str::to_string))
            .collect(),
        None => vec!["claude:claude-opus-5-5".into()],
    }
}

pub fn save_favourites(keys: &[String]) {
    write("favouriteModels", json!(keys));
}

/// Whether the Data Explorer shows its column list and stats.
pub fn column_stats() -> bool {
    read()["deColumnStats"].as_bool().unwrap_or(true)
}

pub fn save_column_stats(shown: bool) {
    write("deColumnStats", json!(shown));
}

/// The folder in a project that tables were last exported to.
pub fn export_folder(project: &str) -> Option<String> {
    read()["exportFolder"][project].as_str().map(str::to_string)
}

pub fn save_export_folder(project: &str, folder: &str) {
    let mut all = read()["exportFolder"].clone();
    if !all.is_object() {
        all = json!({});
    }
    all[project] = json!(folder);
    write("exportFolder", all);
}

/// Whether a project was offered its settings once already.
pub fn settings_asked(project: &str) -> bool {
    read()["settingsAsked"]
        .as_array()
        .is_some_and(|list| list.iter().any(|p| p.as_str() == Some(project)))
}

pub fn save_settings_asked(project: &str) {
    let mut list: Vec<Value> = read()["settingsAsked"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if !list.iter().any(|p| p.as_str() == Some(project)) {
        list.push(json!(project));
    }
    write("settingsAsked", Value::Array(list));
}

/// Whether notebooks' and Quarto documents' runs show in the Console too
/// (the ☰ menu); off, as in the Tauri app.
pub fn notebook_console() -> bool {
    read()["notebookConsole"].as_bool().unwrap_or(false)
}

pub fn save_notebook_console(on: bool) {
    write("notebookConsole", json!(on));
}

/// Whether notebooks' plots show in Plots too, not only under their cells.
pub fn notebook_plots() -> bool {
    read()["notebookPlots"].as_bool().unwrap_or(false)
}

pub fn save_notebook_plots(on: bool) {
    write("notebookPlots", json!(on));
}

/// The Tauri app's data folder, where it keeps the list of servers; the
/// GPUI app reads and writes the same list, so a server added in one shows
/// in the other.
fn tauri_data_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    let base = if cfg!(target_os = "macos") {
        home.join("Library/Application Support")
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share"))
    };
    Some(base.join("dev.sandhill.ide"))
}

/// The servers the Start screen offers next to this Mac: ssh destinations.
pub fn servers() -> Vec<String> {
    tauri_data_dir()
        .and_then(|dir| std::fs::read_to_string(dir.join("servers.json")).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

pub fn save_servers(list: &[String]) -> Result<(), String> {
    let dir = tauri_data_dir().ok_or("no home folder")?;
    std::fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
    let bytes = serde_json::to_vec_pretty(list).map_err(|err| err.to_string())?;
    std::fs::write(dir.join("servers.json"), bytes)
        .map_err(|err| format!("cannot save the list of servers: {err}"))
}

/// The windows open when the app last quit, and the tabs in each, saved
/// next to this file's own settings.
fn windows_file() -> Option<PathBuf> {
    file().map(|f| f.with_file_name("gpui-windows.json"))
}

pub fn windows() -> Vec<crate::app_window::Places> {
    windows_file()
        .and_then(|f| std::fs::read_to_string(f).ok())
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|v| serde_json::from_value(v["windows"].clone()).ok())
        .unwrap_or_default()
}

pub fn save_windows(windows: &[crate::app_window::Places]) {
    let Some(path) = windows_file() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, json!({ "windows": windows }).to_string());
}

/// The machine the Start screen's folder picker last opened a project on.
pub fn start_host() -> Option<String> {
    read()["startHost"].as_str().map(str::to_string)
}

pub fn save_start_host(host: &str) {
    write("startHost", json!(host));
}

/// Whether a section of the left pane (Kernels, Resources) is collapsed.
pub fn section_collapsed(section: &str) -> bool {
    read()["collapsedSections"]
        .as_array()
        .is_some_and(|list| list.iter().any(|s| s.as_str() == Some(section)))
}

pub fn toggle_section(section: &str) {
    let mut list: Vec<Value> = read()["collapsedSections"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if let Some(ix) = list.iter().position(|s| s.as_str() == Some(section)) {
        list.remove(ix);
    } else {
        list.push(json!(section));
    }
    write("collapsedSections", Value::Array(list));
}
