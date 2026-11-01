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
