//! Windows: a start screen, and one window per open project.
//!
//! The projects that are open when the app quits open again at the next
//! launch. Closing the last project window brings back the start screen;
//! closing the start screen on its own quits.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, Runtime, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

pub const START: &str = "start";

/// What a window shows, which the UI asks for when it loads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WindowContext {
    Start,
    Project { host: String, path: String },
}

pub struct Windows {
    /// Window label -> what it shows, in the order the windows opened.
    open: Mutex<Vec<(String, WindowContext)>>,
    /// Remembers the open projects for the next launch.
    saved: PathBuf,
    /// Set by Quit, so windows closing on the way out stay remembered.
    quitting: AtomicBool,
    last_closed_project: AtomicBool,
}

impl Windows {
    pub fn new(saved: PathBuf) -> Self {
        Windows {
            open: Mutex::default(),
            saved,
            quitting: AtomicBool::new(false),
            last_closed_project: AtomicBool::new(false),
        }
    }

    /// Opens the projects that were open at the last quit, or the start screen.
    pub fn restore<R: Runtime>(&self, app: &AppHandle<R>) -> tauri::Result<()> {
        let projects: Vec<(String, String)> = std::fs::read_to_string(&self.saved)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        if projects.is_empty() {
            return self.open_start(app);
        }
        for (host, path) in projects {
            self.open_project(app, &host, &path)?;
        }
        Ok(())
    }

    pub fn open_start<R: Runtime>(&self, app: &AppHandle<R>) -> tauri::Result<()> {
        if let Some(window) = app.get_webview_window(START) {
            return focus(&window);
        }
        let builder = WebviewWindowBuilder::new(app, START, WebviewUrl::App("index.html".into()))
            .title("Sandhill")
            .inner_size(820.0, 600.0)
            .min_inner_size(560.0, 420.0);
        title_bar(builder).build()?;
        self.add(START, WindowContext::Start);
        Ok(())
    }

    pub fn open_project<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        host: &str,
        path: &str,
    ) -> tauri::Result<()> {
        let label = project_label(host, path);
        if let Some(window) = app.get_webview_window(&label) {
            return focus(&window);
        }
        let name = path.rsplit('/').find(|s| !s.is_empty()).unwrap_or(path);
        let title = if host == sandhill_link::LOCAL {
            name.to_string()
        } else {
            format!("{name} — {host}")
        };
        let builder = WebviewWindowBuilder::new(app, &label, WebviewUrl::App("index.html".into()))
            .title(title)
            .inner_size(1280.0, 860.0)
            .min_inner_size(720.0, 480.0);
        title_bar(builder).build()?;
        self.add(
            &label,
            WindowContext::Project {
                host: host.to_string(),
                path: path.to_string(),
            },
        );
        Ok(())
    }

    pub fn context(&self, label: &str) -> Option<WindowContext> {
        let open = self.open.lock().unwrap();
        open.iter()
            .find(|(l, _)| l == label)
            .map(|(_, c)| c.clone())
    }

    /// Called when a window is gone.
    pub fn closed(&self, label: &str) {
        let removed = {
            let mut open = self.open.lock().unwrap();
            let index = open.iter().position(|(l, _)| l == label);
            index.map(|i| open.remove(i).1)
        };
        let was_project = matches!(removed, Some(WindowContext::Project { .. }));
        self.last_closed_project
            .store(was_project, Ordering::SeqCst);
        if was_project && !self.quitting.load(Ordering::SeqCst) {
            self.save();
        }
    }

    pub fn quit<R: Runtime>(&self, app: &AppHandle<R>) {
        self.quitting.store(true, Ordering::SeqCst);
        let windows: Vec<_> = app.webview_windows().into_values().collect();
        if windows.is_empty() {
            app.exit(0);
        }
        // Each window asks about its unsaved documents before it goes.
        for window in windows {
            let _ = window.close();
        }
    }

    pub fn cancel_quit(&self) {
        self.quitting.store(false, Ordering::SeqCst);
    }

    /// Whether to keep running when the last window has closed: closing the
    /// last project shows the start screen instead of quitting.
    pub fn keep_running(&self) -> bool {
        !self.quitting.load(Ordering::SeqCst) && self.last_closed_project.load(Ordering::SeqCst)
    }

    fn add(&self, label: &str, context: WindowContext) {
        let is_project = matches!(context, WindowContext::Project { .. });
        self.open.lock().unwrap().push((label.to_string(), context));
        if is_project {
            self.save();
        }
    }

    fn save(&self) {
        let projects: Vec<(String, String)> = self
            .open
            .lock()
            .unwrap()
            .iter()
            .filter_map(|(_, c)| match c {
                WindowContext::Project { host, path } => Some((host.clone(), path.clone())),
                WindowContext::Start => None,
            })
            .collect();
        let result = serde_json::to_vec_pretty(&projects)
            .map_err(std::io::Error::other)
            .and_then(|bytes| {
                if let Some(dir) = self.saved.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                std::fs::write(&self.saved, bytes)
            });
        if let Err(err) = result {
            tracing::warn!("cannot remember open projects: {err}");
        }
    }
}

/// On macOS the page draws the title bar, under the window buttons, so the
/// panes can float on one background from the top of the window down.
/// The buttons sit in the page's 34px toolbar row: the red one 12px from the
/// left edge and centred on the row, as in the design (docs/design/ui/).
#[cfg(target_os = "macos")]
fn title_bar<'a, R: Runtime, M: Manager<R>>(
    builder: WebviewWindowBuilder<'a, R, M>,
) -> WebviewWindowBuilder<'a, R, M> {
    builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true)
        .traffic_light_position(tauri::LogicalPosition::new(
            TRAFFIC_LIGHTS_X,
            TRAFFIC_LIGHTS_Y,
        ))
}

/// Where the close button's frame starts, in points from the window's top
/// left. Its frame is a point wider than the 12pt light on each side.
#[cfg(target_os = "macos")]
const TRAFFIC_LIGHTS_X: f64 = 11.0;
#[cfg(target_os = "macos")]
const TRAFFIC_LIGHTS_Y: f64 = 10.0;

#[cfg(not(target_os = "macos"))]
fn title_bar<'a, R: Runtime, M: Manager<R>>(
    builder: WebviewWindowBuilder<'a, R, M>,
) -> WebviewWindowBuilder<'a, R, M> {
    builder
}

fn focus<R: Runtime>(window: &WebviewWindow<R>) -> tauri::Result<()> {
    window.unminimize()?;
    window.show()?;
    window.set_focus()
}

/// Window labels allow only `a-zA-Z0-9-/:_`, so projects get a hash.
fn project_label(host: &str, path: &str) -> String {
    let mut hasher = DefaultHasher::new();
    (host, path).hash(&mut hasher);
    format!("project-{:016x}", hasher.finish())
}

/// What the calling window shows.
#[tauri::command]
pub fn window_context<R: Runtime>(
    window: WebviewWindow<R>,
    windows: State<'_, std::sync::Arc<Windows>>,
) -> WindowContext {
    windows
        .context(window.label())
        .unwrap_or(WindowContext::Start)
}

/// Opens a project in its own window, or brings its window to the front.
#[tauri::command]
pub fn open_project_window<R: Runtime>(
    app: AppHandle<R>,
    windows: State<'_, std::sync::Arc<Windows>>,
    host: String,
    path: String,
) -> Result<(), String> {
    windows
        .open_project(&app, &host, &path)
        .map_err(|err| err.to_string())
}

#[tauri::command]
pub fn show_start_window<R: Runtime>(
    app: AppHandle<R>,
    windows: State<'_, std::sync::Arc<Windows>>,
) -> Result<(), String> {
    windows.open_start(&app).map_err(|err| err.to_string())
}

/// A window kept itself open (unsaved changes), so Quit stops.
#[tauri::command]
pub fn cancel_quit(windows: State<'_, std::sync::Arc<Windows>>) {
    windows.cancel_quit();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_labels_are_valid_and_distinct() {
        let a = project_label("local", "/w/a");
        assert!(
            a.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-/:_".contains(&b))
        );
        assert_ne!(a, project_label("local", "/w/b"));
        assert_ne!(a, project_label("ssh:server", "/w/a"));
    }

    #[test]
    fn open_projects_are_remembered_except_while_quitting() {
        let dir = tempfile::tempdir().unwrap();
        let saved = dir.path().join("open-projects.json");
        let windows = Windows::new(saved.clone());
        let read = || -> Vec<(String, String)> {
            serde_json::from_str(&std::fs::read_to_string(&saved).unwrap()).unwrap()
        };
        let project = |path: &str| WindowContext::Project {
            host: "local".into(),
            path: path.into(),
        };
        windows.add("start", WindowContext::Start);
        windows.add("p1", project("/w/a"));
        windows.add("p2", project("/w/b"));
        assert_eq!(read().len(), 2);

        windows.closed("p1");
        assert_eq!(read(), [("local".to_string(), "/w/b".to_string())]);
        assert!(windows.keep_running());

        windows.quitting.store(true, Ordering::SeqCst);
        windows.closed("p2");
        assert_eq!(read().len(), 1, "quitting keeps the list for next time");
        assert!(!windows.keep_running());
    }
}
