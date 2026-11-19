//! The app's own settings on this Mac (or Linux machine), such as starred
//! models. A small JSON file; reading or writing it failing is never an
//! error.

use std::path::PathBuf;

use serde_json::{Value, json};

static SAVES_SUSPENDED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(crate) fn suspend_saves_for_exit() {
    SAVES_SUSPENDED.store(true, std::sync::atomic::Ordering::Relaxed);
}

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
    if SAVES_SUSPENDED.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }
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

pub(crate) fn file_history_limits() -> sandhill_proto::FileHistoryLimits {
    serde_json::from_value(read()["fileHistoryLimits"].clone()).unwrap_or_default()
}

pub(crate) fn save_file_history_limits(limits: sandhill_proto::FileHistoryLimits) {
    write("fileHistoryLimits", json!(limits));
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

/// The explored tables' session folders on this Mac, newest first.
pub(crate) fn quick_sessions() -> Vec<String> {
    read()["quickSessions"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|p| p.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Puts a session folder first in the list.
pub(crate) fn remember_quick_session(path: &str) {
    let mut sessions = quick_sessions();
    sessions.retain(|p| p != path);
    sessions.insert(0, path.to_string());
    sessions.truncate(50);
    write("quickSessions", json!(sessions));
}

pub(crate) fn forget_quick_session(path: &str) {
    let mut sessions = quick_sessions();
    sessions.retain(|p| p != path);
    write("quickSessions", json!(sessions));
}

/// Pins belong to this Mac, scoped by daemon host and absolute project path.
pub(crate) type ProjectPins =
    std::collections::BTreeMap<String, std::collections::BTreeSet<String>>;

pub(crate) fn pinned_projects() -> ProjectPins {
    project_pins_in(&read())
}

fn project_pins_in(all: &Value) -> ProjectPins {
    serde_json::from_value(all["pinnedProjects"].clone()).unwrap_or_default()
}

pub(crate) fn save_pinned_projects(pins: &ProjectPins) {
    write("pinnedProjects", json!(pins));
}

/// This Mac's names for devices, by host (Huan, 2026-11-12). Read on every
/// draw, so kept in memory after the first read.
static DEVICE_NAMES: std::sync::RwLock<Option<std::collections::BTreeMap<String, String>>> =
    std::sync::RwLock::new(None);

pub(crate) fn device_name(host: &str) -> Option<String> {
    if let Some(names) = DEVICE_NAMES.read().unwrap().as_ref() {
        return names.get(host).cloned();
    }
    let names: std::collections::BTreeMap<String, String> =
        serde_json::from_value(read()["deviceNames"].clone()).unwrap_or_default();
    let name = names.get(host).cloned();
    *DEVICE_NAMES.write().unwrap() = Some(names);
    name
}

/// Names a device; an empty name goes back to the default.
pub(crate) fn save_device_name(host: &str, name: &str) {
    device_name(host);
    let mut cached = DEVICE_NAMES.write().unwrap();
    let names = cached.get_or_insert_default();
    match name.trim() {
        "" => names.remove(host),
        name => names.insert(host.to_string(), name.to_string()),
    };
    write("deviceNames", json!(names));
}

/// Agents turned off on Welcome's Agents page, on every device (Huan,
/// 2026-11-15). Read on every draw, so kept in memory. Antigravity starts
/// off: Google suspended an account used with it (2026-11-14).
static AGENTS_OFF: std::sync::RwLock<Option<std::collections::BTreeSet<String>>> =
    std::sync::RwLock::new(None);

fn agents_off_in(all: &Value) -> std::collections::BTreeSet<String> {
    match all.get("agentsOff") {
        Some(off) => serde_json::from_value(off.clone()).unwrap_or_default(),
        None => std::collections::BTreeSet::from(["antigravity".to_string()]),
    }
}

pub(crate) fn agent_off(kind: &str) -> bool {
    if let Some(off) = AGENTS_OFF.read().unwrap().as_ref() {
        return off.contains(kind);
    }
    let off = agents_off_in(&read());
    let is_off = off.contains(kind);
    *AGENTS_OFF.write().unwrap() = Some(off);
    is_off
}

pub(crate) fn save_agent_off(kind: &str, off: bool) {
    agent_off(kind);
    let mut cached = AGENTS_OFF.write().unwrap();
    let all = cached.get_or_insert_default();
    if off {
        all.insert(kind.to_string());
    } else {
        all.remove(kind);
    }
    write("agentsOff", json!(all));
}

/// Whether the status bar shows, and the daemon is asked for, the device's
/// CPU, memory, GPU and disk (Huan, 2026-11-15): hidden, nothing is
/// measured. On this Mac for every device, shown at first.
static RESOURCES_SHOWN: std::sync::RwLock<Option<bool>> = std::sync::RwLock::new(None);

pub(crate) fn resources_shown() -> bool {
    if let Some(shown) = *RESOURCES_SHOWN.read().unwrap() {
        return shown;
    }
    let shown = read()["resourcesShown"].as_bool().unwrap_or(true);
    *RESOURCES_SHOWN.write().unwrap() = Some(shown);
    shown
}

pub(crate) fn save_resources_shown(shown: bool) {
    *RESOURCES_SHOWN.write().unwrap() = Some(shown);
    write("resourcesShown", json!(shown));
}

/// Whether the status bar has the device's usage at all, numbers and
/// chevron (Huan, 2026-11-16: the device popup's switch). Off, nothing is
/// measured while the popup is closed. On this Mac for every device.
static USAGE_IN_STATUS_BAR: std::sync::RwLock<Option<bool>> = std::sync::RwLock::new(None);

pub(crate) fn usage_in_status_bar() -> bool {
    if let Some(on) = *USAGE_IN_STATUS_BAR.read().unwrap() {
        return on;
    }
    let on = read()["usageInStatusBar"].as_bool().unwrap_or(true);
    *USAGE_IN_STATUS_BAR.write().unwrap() = Some(on);
    on
}

pub(crate) fn save_usage_in_status_bar(on: bool) {
    *USAGE_IN_STATUS_BAR.write().unwrap() = Some(on);
    write("usageInStatusBar", json!(on));
}

/// Whether the window is frosted glass (the default) or solid (Huan,
/// 2026-11-16: a choice on Welcome's General page). Read on every palette
/// change, so cached.
static GLASS: std::sync::RwLock<Option<bool>> = std::sync::RwLock::new(None);

pub(crate) fn glass() -> bool {
    if let Some(on) = *GLASS.read().unwrap() {
        return on;
    }
    let on = read()["glass"].as_bool().unwrap_or(true);
    *GLASS.write().unwrap() = Some(on);
    on
}

pub(crate) fn save_glass(on: bool) {
    *GLASS.write().unwrap() = Some(on);
    write("glass", json!(on));
}

/// The height an input box was dragged to (`"console"`, `"agent"`); `None`
/// is its own size (Huan, 2026-11-16).
pub(crate) fn input_height(which: &str) -> Option<f32> {
    read()["inputHeights"][which].as_f64().map(|h| h as f32)
}

pub(crate) fn save_input_height(which: &str, height: Option<f32>) {
    let mut all = read()["inputHeights"].clone();
    if !all.is_object() {
        all = json!({});
    }
    match height {
        Some(h) => all[which] = json!(h.round()),
        None => {
            if let Some(map) = all.as_object_mut() {
                map.remove(which);
            }
        }
    }
    write("inputHeights", all);
}

/// Icons chosen for projects: host → folder → icon path such as
/// `folders/folder-src` (Huan, 2026-11-12). Kept in memory like names.
pub(crate) type ProjectIcons =
    std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>>;

static PROJECT_ICONS: std::sync::RwLock<Option<ProjectIcons>> = std::sync::RwLock::new(None);

pub(crate) fn project_icon(host: &str, path: &str) -> Option<String> {
    if let Some(icons) = PROJECT_ICONS.read().unwrap().as_ref() {
        return icons.get(host).and_then(|m| m.get(path)).cloned();
    }
    let icons: ProjectIcons =
        serde_json::from_value(read()["projectIcons"].clone()).unwrap_or_default();
    let icon = icons.get(host).and_then(|m| m.get(path)).cloned();
    *PROJECT_ICONS.write().unwrap() = Some(icons);
    icon
}

/// Sets a project's icon; `None` goes back to the default.
pub(crate) fn save_project_icon(host: &str, path: &str, icon: Option<&str>) {
    project_icon(host, path);
    let mut cached = PROJECT_ICONS.write().unwrap();
    let icons = cached.get_or_insert_default();
    let device = icons.entry(host.to_string()).or_default();
    match icon {
        Some(icon) => device.insert(path.to_string(), icon.to_string()),
        None => device.remove(path),
    };
    if device.is_empty() {
        icons.remove(host);
    }
    write("projectIcons", json!(icons));
}

/// How a text file is coloured when chosen in its editor rather than taken
/// from its type (Huan, 2026-11-14), per device and file, on this Mac only.
static TEXT_COLOURINGS: std::sync::RwLock<Option<ProjectIcons>> = std::sync::RwLock::new(None);

pub(crate) fn text_colouring(host: &str, path: &str) -> Option<String> {
    if let Some(choices) = TEXT_COLOURINGS.read().unwrap().as_ref() {
        return choices.get(host).and_then(|m| m.get(path)).cloned();
    }
    let choices: ProjectIcons =
        serde_json::from_value(read()["textColourings"].clone()).unwrap_or_default();
    let choice = choices.get(host).and_then(|m| m.get(path)).cloned();
    *TEXT_COLOURINGS.write().unwrap() = Some(choices);
    choice
}

/// `None` goes back to the file type's own colouring.
pub(crate) fn save_text_colouring(host: &str, path: &str, choice: Option<&str>) {
    text_colouring(host, path);
    let mut cached = TEXT_COLOURINGS.write().unwrap();
    let choices = cached.get_or_insert_default();
    let device = choices.entry(host.to_string()).or_default();
    match choice {
        Some(choice) => device.insert(path.to_string(), choice.to_string()),
        None => device.remove(path),
    };
    if device.is_empty() {
        choices.remove(host);
    }
    write("textColourings", json!(choices));
}

#[cfg(test)]
mod local_settings_tests {
    use super::{ProjectPins, editor_soft_wrap_in, project_pins_in};
    use serde_json::json;

    #[test]
    fn pins_round_trip_without_crossing_devices() {
        let pins = ProjectPins::from([
            ("local".into(), ["/work/a".into()].into()),
            ("lab".into(), ["/work/a".into(), "/work/b".into()].into()),
        ]);
        let text = json!({"pinnedProjects": pins}).to_string();
        let restored = project_pins_in(&serde_json::from_str(&text).unwrap());
        assert_eq!(restored, pins);
        assert!(!restored["local"].contains("/work/b"));
        assert!(project_pins_in(&json!({})).is_empty());
    }

    #[test]
    fn editor_wrap_defaults_and_explicit_overrides() {
        let missing = json!({});
        for kind in ["markdown", "text"] {
            assert!(editor_soft_wrap_in(&missing, kind));
        }
        for kind in ["r", "python", "bash", "json", "yaml", "toml", "code"] {
            assert!(!editor_soft_wrap_in(&missing, kind));
        }
        let saved = json!({"markdown": false, "python": true, "text": "invalid"});
        assert!(!editor_soft_wrap_in(&saved, "markdown"));
        assert!(editor_soft_wrap_in(&saved, "python"));
        assert!(editor_soft_wrap_in(&saved, "text"));
    }
}

/// Whether a notebook scrolls to, and lights up, the cells an agent adds
/// or changes (the switch in the AI pane).
pub fn follow_agent() -> bool {
    follow_agent_in(&read())
}

fn follow_agent_in(all: &Value) -> bool {
    all["followAgent"].as_bool().unwrap_or(true)
}

pub fn save_follow_agent(on: bool) {
    write("followAgent", json!(on));
}

/// Document wrapping is local to this Mac, shared by editor language.
static EDITOR_SOFT_WRAP: std::sync::RwLock<Option<Value>> = std::sync::RwLock::new(None);

fn editor_soft_wrap_in(choices: &Value, kind: &str) -> bool {
    choices[kind]
        .as_bool()
        .unwrap_or(matches!(kind, "markdown" | "text"))
}

pub(crate) fn editor_soft_wrap(kind: &str) -> bool {
    if let Some(choices) = EDITOR_SOFT_WRAP.read().unwrap().as_ref() {
        return editor_soft_wrap_in(choices, kind);
    }
    let mut choices = read()["editorSoftWrap"].clone();
    if !choices.is_object() {
        choices = json!({});
    }
    let on = editor_soft_wrap_in(&choices, kind);
    *EDITOR_SOFT_WRAP.write().unwrap() = Some(choices);
    on
}

pub(crate) fn save_editor_soft_wrap(kind: &str, on: bool) {
    editor_soft_wrap(kind);
    let mut cached = EDITOR_SOFT_WRAP.write().unwrap();
    let choices = cached.get_or_insert_with(|| json!({}));
    choices[kind] = json!(on);
    write("editorSoftWrap", choices.clone());
}

/// Provider display/polling choices on a connected device. Missing entries
/// remain enabled, including providers added after this settings file was saved.
pub(crate) fn quota_providers(host: &str) -> std::collections::BTreeMap<String, bool> {
    quota_providers_in(&read(), host)
}

fn quota_providers_in(all: &Value, host: &str) -> std::collections::BTreeMap<String, bool> {
    all["quotaProviders"][host]
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(kind, on)| on.as_bool().map(|on| (kind.clone(), on)))
        .collect()
}

pub(crate) fn save_quota_provider(host: &str, kind: &str, on: bool) {
    let mut devices = read()["quotaProviders"].clone();
    if !devices.is_object() {
        devices = json!({});
    }
    if !devices[host].is_object() {
        devices[host] = json!({});
    }
    devices[host][kind] = json!(on);
    write("quotaProviders", devices);
}

/// The account new conversations of an agent start with on a device, as set
/// by the quota panel's "Use for new chats" (switching a conversation keeps it).
pub(crate) fn agent_account(host: &str, kind: &str) -> Option<String> {
    agent_account_in(&read(), host, kind)
}

fn agent_account_in(all: &Value, host: &str, kind: &str) -> Option<String> {
    all["agentAccounts"][host][kind]
        .as_str()
        .map(str::to_string)
}

pub(crate) fn save_agent_account(host: &str, kind: &str, account: &str) {
    let mut devices = read()["agentAccounts"].clone();
    if !devices.is_object() {
        devices = json!({});
    }
    if !devices[host].is_object() {
        devices[host] = json!({});
    }
    devices[host][kind] = json!(account);
    write("agentAccounts", devices);
}

pub(crate) fn forget_agent_account(host: &str, kind: &str, account: &str) {
    let mut devices = read()["agentAccounts"].clone();
    if devices[host][kind].as_str() == Some(account)
        && let Some(accounts) = devices[host].as_object_mut()
    {
        accounts.remove(kind);
        write("agentAccounts", devices);
    }
}

#[cfg(test)]
mod account_tests {
    use super::agent_account_in;
    use serde_json::json;

    #[test]
    fn account_choices_are_device_and_agent_scoped() {
        let saved = json!({"agentAccounts": {
            "local": {"claude": "claude:work", "codex": "codex"},
            "lab": {"claude": "claude:lab"}
        }});
        assert_eq!(
            agent_account_in(&saved, "local", "claude").as_deref(),
            Some("claude:work")
        );
        assert_eq!(
            agent_account_in(&saved, "lab", "claude").as_deref(),
            Some("claude:lab")
        );
        assert_eq!(
            agent_account_in(&saved, "local", "codex").as_deref(),
            Some("codex")
        );
        assert_eq!(agent_account_in(&saved, "lab", "codex"), None);
        assert_eq!(agent_account_in(&saved, "other", "claude"), None);
    }
}

/// Whether a notebook suggests the next cell after each run (ghost cells).
pub fn ghost_cells() -> bool {
    ghost_cells_in(&read())
}

fn ghost_cells_in(all: &Value) -> bool {
    all["ghostCells"].as_bool().unwrap_or(false)
}

pub fn save_ghost_cells(on: bool) {
    write("ghostCells", json!(on));
}

/// Whether the Data Explorer shows its column list and stats.
pub fn column_stats() -> bool {
    column_stats_in(&read())
}

fn column_stats_in(all: &Value) -> bool {
    all["deColumnStats"].as_bool().unwrap_or(true)
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
    notebook_console_in(&read())
}

fn notebook_console_in(all: &Value) -> bool {
    all["notebookConsole"].as_bool().unwrap_or(false)
}

pub fn save_notebook_console(on: bool) {
    write("notebookConsole", json!(on));
}

/// Whether notebooks' plots show in Plots too, not only under their cells.
pub fn notebook_plots() -> bool {
    notebook_plots_in(&read())
}

fn notebook_plots_in(all: &Value) -> bool {
    all["notebookPlots"].as_bool().unwrap_or(false)
}

pub fn save_notebook_plots(on: bool) {
    write("notebookPlots", json!(on));
}

/// Console forwarding is opt-in on each daemon device.
pub(crate) fn console_plots(host: &str) -> bool {
    read()["consolePlots"][host].as_bool().unwrap_or(false)
}

pub(crate) fn save_console_plots(host: &str, on: bool) {
    let mut devices = read()["consolePlots"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    devices.insert(host.to_string(), json!(on));
    write("consolePlots", json!(devices));
}

/// Oldest first, bounded across devices, sessions and immutable plot identities.
pub(crate) type SentPlots = Vec<[String; 3]>;
const SENT_PLOTS: usize = 3000;

pub(crate) fn sent_plots() -> SentPlots {
    let mut sent: SentPlots =
        serde_json::from_value(read()["sentConsolePlots"].clone()).unwrap_or_default();
    if sent.len() > SENT_PLOTS {
        sent.drain(..sent.len() - SENT_PLOTS);
    }
    sent
}

pub(crate) fn remember_plot(sent: &mut SentPlots, host: &str, session: &str, key: &str) -> bool {
    if sent
        .iter()
        .any(|entry| entry[0] == host && entry[1] == session && entry[2] == key)
    {
        return false;
    }
    sent.push([host.to_string(), session.to_string(), key.to_string()]);
    if sent.len() > SENT_PLOTS {
        sent.drain(..sent.len() - SENT_PLOTS);
    }
    true
}

pub(crate) fn save_sent_plots(sent: &SentPlots) {
    write("sentConsolePlots", json!(sent));
}

#[cfg(test)]
mod sent_plot_tests {
    use super::{SENT_PLOTS, remember_plot};

    #[test]
    fn identities_are_device_and_session_scoped_and_bounded() {
        let mut sent = Vec::new();
        assert!(remember_plot(&mut sent, "local", "s1", "p"));
        assert!(!remember_plot(&mut sent, "local", "s1", "p"));
        assert!(remember_plot(&mut sent, "remote", "s1", "p"));
        assert!(remember_plot(&mut sent, "local", "s2", "p"));
        for n in 0..SENT_PLOTS {
            remember_plot(&mut sent, "local", "s3", &n.to_string());
        }
        assert_eq!(sent.len(), SENT_PLOTS);
        assert_eq!(sent[0][2], "0");
        assert_eq!(sent.last().unwrap()[2], (SENT_PLOTS - 1).to_string());
    }
}

/// The app's data folder, named for its bundle id, where the list of
/// servers is kept (where the earlier, Tauri app kept it too).
fn data_dir() -> Option<PathBuf> {
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
    data_dir()
        .and_then(|dir| std::fs::read_to_string(dir.join("servers.json")).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// Per-server automatic Welcome connections. Missing entries default to on.
pub(crate) fn server_auto_connect() -> std::collections::BTreeMap<String, bool> {
    serde_json::from_value(read()["serverAutoConnect"].clone()).unwrap_or_default()
}

pub(crate) fn save_server_auto_connect(servers: &std::collections::BTreeMap<String, bool>) {
    write("serverAutoConnect", json!(servers));
}

pub fn save_servers(list: &[String]) -> Result<(), String> {
    if SAVES_SUSPENDED.load(std::sync::atomic::Ordering::Relaxed) {
        return Err("Sandhill is closing after settings cleanup".into());
    }
    let dir = data_dir().ok_or("no home folder")?;
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

pub fn windows() -> Vec<crate::shell::app_window::Places> {
    windows_file()
        .and_then(|f| std::fs::read_to_string(f).ok())
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|v| serde_json::from_value(v["windows"].clone()).ok())
        .unwrap_or_default()
}

pub fn save_windows(windows: &[crate::shell::app_window::Places]) {
    if SAVES_SUSPENDED.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let Some(path) = windows_file() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, json!({ "windows": windows }).to_string());
}

/// The theme picked on General or with the title bar's button, "light" or
/// "dark"; none follows the system.
pub fn appearance() -> Option<String> {
    appearance_in(&read())
}

fn appearance_in(all: &Value) -> Option<String> {
    all["appearance"].as_str().map(str::to_string)
}

/// `None` follows the system again.
pub fn save_appearance(theme: Option<&str>) {
    write(
        "appearance",
        theme.map_or(Value::Null, |theme| json!(theme)),
    );
}

/// The code font and the two text sizes picked on General (Huan,
/// 2026-11-18). A missing or odd value keeps its default.
pub fn typography() -> crate::look::Typography {
    typography_in(&read())
}

fn typography_in(all: &Value) -> crate::look::Typography {
    let fonts = &all["fonts"];
    let size = |key: &str| {
        fonts[key]
            .as_f64()
            .map_or(crate::look::DEFAULT_SIZE, |size| size as f32)
    };
    crate::look::Typography {
        code_font: fonts["code"]
            .as_str()
            .filter(|font| !font.is_empty())
            .map(str::to_string),
        code_size: size("codeSize"),
        ui_size: size("interfaceSize"),
        missing: None,
    }
}

pub fn save_typography(picked: &crate::look::Typography) {
    write(
        "fonts",
        json!({
            "code": picked.code_font,
            "codeSize": picked.code_size,
            "interfaceSize": picked.ui_size,
        }),
    );
}

/// How the files pane sorts: "name", "size" or "modified".
pub fn tree_sort() -> String {
    tree_sort_in(&read())
}

fn tree_sort_in(all: &Value) -> String {
    all["treeSort"].as_str().unwrap_or("name").to_string()
}

pub fn save_tree_sort(by: &str) {
    write("treeSort", json!(by));
}

/// How a table in the Data Explorer was last sorted and filtered, in a
/// project: `{sort: [...], filters: [...]}`, each key with its column's name.
pub fn table_view(project: &str, table: &str) -> Option<Value> {
    let view = read()["tableViews"][project][table].clone();
    view.is_object().then_some(view)
}

/// Keeps a table's sorting and filters, or forgets them (`None`).
pub fn save_table_view(project: &str, table: &str, view: Option<Value>) {
    let mut all = read()["tableViews"].clone();
    if !all.is_object() {
        all = json!({});
    }
    if !all[project].is_object() {
        all[project] = json!({});
    }
    if let Some(tables) = all[project].as_object_mut() {
        match view {
            Some(view) => {
                tables.insert(table.to_string(), view);
            }
            None => {
                tables.remove(table);
            }
        }
    }
    write("tableViews", all);
}

/// Physical plot size, scoped to the daemon host and project, not a file.
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub(crate) enum PlotSizing {
    #[default]
    Fit,
    Fixed {
        width: f64,
        height: f64,
        dpi: u32,
    },
}

impl PlotSizing {
    pub(crate) fn pixels(self) -> Option<(u32, u32)> {
        match self {
            Self::Fixed { width, height, dpi }
                if width.is_finite()
                    && height.is_finite()
                    && width > 0.
                    && height > 0.
                    && dpi > 0
                    && width * f64::from(dpi) <= 30000.
                    && height * f64::from(dpi) <= 30000. =>
            {
                Some((
                    (width * f64::from(dpi)).round().max(1.) as u32,
                    (height * f64::from(dpi)).round().max(1.) as u32,
                ))
            }
            _ => None,
        }
    }
}

/// Export controls extend the existing plot-sizing record without changing its
/// tagged fit/fixed representation, so older preferences still load.
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PlotUnit {
    Pixels,
    #[default]
    Inches,
    Centimetres,
}

impl PlotUnit {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Pixels => "px",
            Self::Inches => "in",
            Self::Centimetres => "cm",
        }
    }

    pub(crate) fn display_value(self, inches: f64, dpi: u32) -> f64 {
        inches
            * match self {
                Self::Pixels => f64::from(dpi),
                Self::Inches => 1.,
                Self::Centimetres => 2.54,
            }
    }

    pub(crate) fn inches(self, value: f64, dpi: u32) -> f64 {
        value / self.display_value(1., dpi)
    }
}

pub(crate) fn plot_export(host: &str, project: &str) -> (PlotUnit, u32) {
    let all = read();
    let entry = &all["plotSizing"][host][project];
    let unit = serde_json::from_value(entry["unit"].clone()).unwrap_or_default();
    let dpi = entry["exportDpi"]
        .as_u64()
        .or_else(|| entry["dpi"].as_u64())
        .filter(|dpi| *dpi > 0 && *dpi <= 30000)
        .unwrap_or(300) as u32;
    (unit, dpi)
}

pub(crate) fn save_plot_export(host: &str, project: &str, unit: PlotUnit, dpi: u32) {
    save_plot_sizing_fields(host, project, json!({"unit": unit, "exportDpi": dpi}));
}

/// Whether a PDF is saved rasterized (Huan, 2026-11-14), per project.
pub(crate) fn plot_rasterize(host: &str, project: &str) -> bool {
    read()["plotSizing"][host][project]["rasterize"]
        .as_bool()
        .unwrap_or(false)
}

pub(crate) fn save_plot_rasterize(host: &str, project: &str, on: bool) {
    save_plot_sizing_fields(host, project, json!({"rasterize": on}));
}

/// Whether the plot window shows its settings column (Huan, 2026-11-14);
/// one choice on this Mac, shown at first.
pub(crate) fn plot_settings_shown() -> bool {
    plot_settings_shown_in(&read())
}

fn plot_settings_shown_in(all: &Value) -> bool {
    all["plotSettingsShown"].as_bool().unwrap_or(true)
}

pub(crate) fn save_plot_settings_shown(on: bool) {
    write("plotSettingsShown", json!(on));
}

/// Whether the plot save window also saves a recipe, off at first.
pub(crate) fn plot_recipe() -> bool {
    read()["plotRecipe"].as_bool().unwrap_or(false)
}

pub(crate) fn save_plot_recipe(on: bool) {
    write("plotRecipe", json!(on));
}

/// The choices for every project on this Mac that Welcome's General page
/// shows (Huan, 2026-11-15), read in one go. Wrapping is per file type:
/// `editor_soft_wrap`.
pub(crate) struct General {
    pub appearance: Option<String>,
    pub notebook_console: bool,
    pub notebook_plots: bool,
    pub ghost_cells: bool,
    pub follow_agent: bool,
    pub tree_sort: String,
    pub plot_settings: bool,
    pub column_panel: bool,
}

pub(crate) fn general() -> General {
    let all = read();
    General {
        appearance: appearance_in(&all),
        notebook_console: notebook_console_in(&all),
        notebook_plots: notebook_plots_in(&all),
        ghost_cells: ghost_cells_in(&all),
        follow_agent: follow_agent_in(&all),
        tree_sort: tree_sort_in(&all),
        plot_settings: plot_settings_shown_in(&all),
        column_panel: column_stats_in(&all),
    }
}

pub(crate) fn plot_folder(host: &str, project: &str) -> Option<String> {
    read()["plotFolder"][host][project]
        .as_str()
        .map(str::to_string)
}

pub(crate) fn save_plot_folder(host: &str, project: &str, folder: &str) {
    let mut all = read()["plotFolder"].clone();
    if !all.is_object() {
        all = json!({});
    }
    if !all[host].is_object() {
        all[host] = json!({});
    }
    all[host][project] = json!(folder);
    write("plotFolder", all);
}

fn plot_sizing_in(all: &Value, host: &str, project: &str) -> PlotSizing {
    let sizing =
        serde_json::from_value(all["plotSizing"][host][project].clone()).unwrap_or_default();
    if matches!(sizing, PlotSizing::Fit) || sizing.pixels().is_some() {
        sizing
    } else {
        PlotSizing::Fit
    }
}

pub(crate) fn plot_sizing(host: &str, project: &str) -> PlotSizing {
    plot_sizing_in(&read(), host, project)
}

pub(crate) fn save_plot_sizing(host: &str, project: &str, sizing: PlotSizing) {
    save_plot_sizing_fields(host, project, json!(sizing));
}

fn save_plot_sizing_fields(host: &str, project: &str, fields: Value) {
    let mut all = read()["plotSizing"].clone();
    if !all.is_object() {
        all = json!({});
    }
    if !all[host].is_object() {
        all[host] = json!({});
    }
    if !all[host][project].is_object() {
        all[host][project] = json!({"mode": "fit"});
    }
    all[host][project]
        .as_object_mut()
        .unwrap()
        .extend(fields.as_object().unwrap().clone());
    write("plotSizing", all);
}

#[cfg(test)]
mod plot_sizing_tests {
    use super::{PlotSizing, plot_sizing_in};
    use serde_json::json;

    #[test]
    fn extended_export_fields_leave_old_sizing_compatible() {
        let all = json!({"plotSizing": {"host": {"/p": {
            "mode": "fixed", "width": 6.4, "height": 4.8, "dpi": 300,
            "unit": "centimetres", "exportDpi": 150
        }}}});
        assert_eq!(
            plot_sizing_in(&all, "host", "/p"),
            PlotSizing::Fixed {
                width: 6.4,
                height: 4.8,
                dpi: 300,
            }
        );
        let old = json!({"plotSizing": {"host": {"/p": {"mode": "fit"}}}});
        assert_eq!(plot_sizing_in(&old, "host", "/p"), PlotSizing::Fit);
    }

    #[test]
    fn physical_size_computes_pixels_and_round_trips_per_host_project() {
        let sizing = PlotSizing::Fixed {
            width: 3.5,
            height: 3.,
            dpi: 300,
        };
        assert_eq!(sizing.pixels(), Some((1050, 900)));
        let all = json!({"plotSizing": {"local": {"/p": sizing}}});
        let back = serde_json::from_str(&all.to_string()).unwrap();
        assert_eq!(plot_sizing_in(&back, "local", "/p"), sizing);
        assert_eq!(plot_sizing_in(&back, "remote", "/p"), PlotSizing::Fit);
        assert_eq!(plot_sizing_in(&back, "local", "/q"), PlotSizing::Fit);
        assert_eq!(
            PlotSizing::Fixed {
                width: 7.,
                height: 5.,
                dpi: 300
            }
            .pixels(),
            Some((2100, 1500))
        );
        assert!(
            PlotSizing::Fixed {
                width: f64::NAN,
                height: 3.,
                dpi: 300
            }
            .pixels()
            .is_none()
        );
        assert!(
            PlotSizing::Fixed {
                width: 3.,
                height: 0.,
                dpi: 300
            }
            .pixels()
            .is_none()
        );
    }
}

fn owned_files() -> Vec<PathBuf> {
    [
        file(),
        windows_file(),
        data_dir().map(|dir| dir.join("servers.json")),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn safe_regular_file(path: &std::path::Path) -> Result<u64, String> {
    for ancestor in path.ancestors() {
        match std::fs::symlink_metadata(ancestor) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err("Symbolic links are protected".into());
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    let meta = std::fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if !meta.is_file() {
        return Err("Not a regular settings file".into());
    }
    Ok(meta.len())
}

fn measured_bytes(path: &std::path::Path) -> Result<u64, String> {
    for ancestor in path.ancestors() {
        if std::fs::symlink_metadata(ancestor)
            .map_err(|error| error.to_string())?
            .file_type()
            .is_symlink()
        {
            return Err("Symbolic links are not followed".into());
        }
    }
    let meta = std::fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if meta.is_file() {
        return Ok(meta.len());
    }
    if !meta.is_dir() {
        return Err("Unsupported app-support entry".into());
    }
    let mut bytes = 0_u64;
    for entry in std::fs::read_dir(path).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        bytes = bytes.saturating_add(measured_bytes(&entry.path())?);
    }
    Ok(bytes)
}

/// Only known app settings files are removable. Unknown neighboring files
/// remain visible and protected; neither application-support root is deleted.
pub(crate) fn storage_inventory() -> Value {
    let owned = owned_files();
    let roots: std::collections::BTreeSet<_> = owned
        .iter()
        .filter_map(|path| path.parent().map(std::path::Path::to_path_buf))
        .collect();
    let mut paths: std::collections::BTreeSet<_> = owned.iter().cloned().collect();
    let mut errors = Vec::new();
    if owned.is_empty() {
        errors.push("Cannot locate the app settings folder: no home directory".into());
    }
    for root in roots {
        if root.ancestors().any(|ancestor| {
            std::fs::symlink_metadata(ancestor).is_ok_and(|meta| meta.file_type().is_symlink())
        }) {
            errors.push(format!("Protected symbolic-link path: {}", root.display()));
            continue;
        }
        match std::fs::read_dir(&root) {
            Ok(entries) => {
                for entry in entries {
                    match entry {
                        Ok(entry) => {
                            paths.insert(entry.path());
                        }
                        Err(error) => errors.push(error.to_string()),
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => errors.push(format!("{}: {error}", root.display())),
        }
    }
    let mut entries = Vec::new();
    for path in paths {
        if std::fs::symlink_metadata(&path)
            .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
        {
            continue;
        }
        let inspected = safe_regular_file(&path);
        let measured = measured_bytes(&path);
        if let Err(error) = &measured {
            errors.push(format!("{}: size unknown: {error}", path.display()));
        }
        let removable = owned.contains(&path) && inspected.is_ok();
        let reason = if !owned.contains(&path) {
            Some("Unknown app-support content is protected".to_string())
        } else {
            inspected.as_ref().err().cloned()
        };
        entries.push(json!({
            "id": path.to_string_lossy(), "category": "app_settings",
            "path": path.to_string_lossy(), "bytes": measured.unwrap_or(0),
            "removable": removable, "reason": reason,
        }));
    }
    json!({"entries": entries, "errors": errors})
}

/// The caller must save edits first, run remote operations first, then close
/// the application and suspend preference writes only after successful cleanup.
pub(crate) fn clean_storage() -> Value {
    let mut results = Vec::new();
    let mut errors = Vec::new();
    if owned_files().is_empty() {
        errors.push("Cannot locate the app settings folder: no home directory".into());
    }
    for path in owned_files() {
        if std::fs::symlink_metadata(&path)
            .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
        {
            continue;
        }
        let result = safe_regular_file(&path)
            .and_then(|_| std::fs::remove_file(&path).map_err(|error| error.to_string()));
        if let Err(error) = &result {
            errors.push(format!("{}: {error}", path.display()));
        }
        results.push(json!({"path": path.to_string_lossy(), "removed": result.is_ok(), "error": result.err()}));
    }
    json!({"success": errors.is_empty(), "results": results, "errors": errors})
}

#[cfg(test)]
mod storage_tests {
    #[test]
    fn quota_preferences_survive_json_reload_and_are_device_scoped() {
        let saved = json!({"quotaProviders": {
            "local": {"claude": false, "codex": true},
            "remote": {"codex": false}
        }});
        let reloaded: Value = serde_json::from_str(&saved.to_string()).unwrap();
        let local = quota_providers_in(&reloaded, "local");
        assert_eq!(local.get("claude"), Some(&false));
        assert_eq!(local.get("codex"), Some(&true));
        assert!(local.get("antigravity").copied().unwrap_or(true));
        assert_eq!(
            quota_providers_in(&reloaded, "remote").get("codex"),
            Some(&false)
        );
        assert!(quota_providers_in(&reloaded, "new-device").is_empty());
    }

    use super::{measured_bytes, quota_providers_in, safe_regular_file};
    use serde_json::{Value, json};

    #[test]
    fn measures_owned_tree_without_following_symlinks() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "sandhill-prefs-test-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        std::fs::write(root.join("gpui.json"), b"{}").unwrap();
        assert_eq!(safe_regular_file(&root.join("gpui.json")).unwrap(), 2);
        assert_eq!(measured_bytes(&root).unwrap(), 2);
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(root.join("gpui.json"), root.join("linked.json")).unwrap();
            assert!(safe_regular_file(&root.join("linked.json")).is_err());
            assert!(measured_bytes(&root).is_err());
            std::fs::remove_file(root.join("linked.json")).unwrap();
        }
        std::fs::remove_file(root.join("gpui.json")).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
}
