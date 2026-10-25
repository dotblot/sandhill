//! Windows. Each has a tab per machine, with one project open on each, or
//! shows the Start screen (`app/src/ui/Workbench.tsx` draws them).
//!
//! What the windows have open when the app quits opens again at the next
//! launch. Closing the last window that had projects brings back an empty
//! one with the Start screen; closing that one quits.

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, Runtime, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

/// A project on a machine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Place {
    pub host: String,
    pub path: String,
}

/// What a window has open, which the UI asks for when it loads and reports
/// as it changes.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WindowPlaces {
    /// One tab per machine, in order.
    pub tabs: Vec<Place>,
    /// The machine shown, or none for the Start screen.
    pub current: Option<String>,
}

/// The file the open windows are remembered in.
#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum Saved {
    Windows {
        windows: Vec<WindowPlaces>,
    },
    /// Before machine tabs (2026-10-25): one project per window.
    Projects(Vec<(String, String)>),
}

impl Saved {
    fn windows(self) -> Vec<WindowPlaces> {
        match self {
            Saved::Windows { windows } => windows,
            // One window now holds them: a tab per machine, with the project
            // listed last on each, as the window opened last.
            Saved::Projects(projects) => {
                let mut window = WindowPlaces::default();
                for (host, path) in projects {
                    window.tabs.retain(|t| t.host != host);
                    window.tabs.push(Place {
                        host: host.clone(),
                        path,
                    });
                    window.current = Some(host);
                }
                if window.tabs.is_empty() {
                    vec![]
                } else {
                    vec![window]
                }
            }
        }
    }
}

pub struct Windows {
    /// Window label -> what it has open, in the order the windows opened.
    open: Mutex<Vec<(String, WindowPlaces)>>,
    /// Remembers the open windows for the next launch.
    saved: PathBuf,
    /// Set by Quit, so windows closing on the way out stay remembered.
    quitting: AtomicBool,
    /// Whether the window that closed last had projects open.
    last_closed_had_tabs: AtomicBool,
    next_label: AtomicUsize,
}

impl Windows {
    pub fn new(saved: PathBuf) -> Self {
        Windows {
            open: Mutex::default(),
            saved,
            quitting: AtomicBool::new(false),
            last_closed_had_tabs: AtomicBool::new(false),
            next_label: AtomicUsize::new(1),
        }
    }

    /// Opens the windows that were open at the last quit, or an empty one.
    pub fn restore<R: Runtime>(&self, app: &AppHandle<R>) -> tauri::Result<()> {
        let windows = std::fs::read_to_string(&self.saved)
            .ok()
            .and_then(|text| serde_json::from_str::<Saved>(&text).ok())
            .map(Saved::windows)
            .unwrap_or_default();
        if windows.is_empty() {
            return self.open_window(app, WindowPlaces::default());
        }
        for places in windows {
            self.open_window(app, places)?;
        }
        Ok(())
    }

    pub fn open_window<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        places: WindowPlaces,
    ) -> tauri::Result<()> {
        let label = format!("window-{}", self.next_label.fetch_add(1, Ordering::SeqCst));
        let builder = WebviewWindowBuilder::new(app, &label, WebviewUrl::App("index.html".into()))
            .title(title(&places))
            .inner_size(1280.0, 860.0)
            .min_inner_size(720.0, 480.0)
            // Files dragged in from Finder reach the page as HTML drops (the
            // Agents pane's message box takes them), not as Tauri events.
            .disable_drag_drop_handler();
        title_bar(builder).build()?;
        self.set(&label, places);
        Ok(())
    }

    pub fn places(&self, label: &str) -> Option<WindowPlaces> {
        let open = self.open.lock().unwrap();
        open.iter()
            .find(|(l, _)| l == label)
            .map(|(_, p)| p.clone())
    }

    /// Records what a window has open, and remembers it for the next launch.
    fn set(&self, label: &str, places: WindowPlaces) {
        {
            let mut open = self.open.lock().unwrap();
            match open.iter_mut().find(|(l, _)| l == label) {
                Some((_, old)) => *old = places,
                None => open.push((label.to_string(), places)),
            }
        }
        self.save();
    }

    /// Called when a window is gone.
    pub fn closed(&self, label: &str) {
        let removed = {
            let mut open = self.open.lock().unwrap();
            let index = open.iter().position(|(l, _)| l == label);
            index.map(|i| open.remove(i).1)
        };
        let had_tabs = removed.is_some_and(|p| !p.tabs.is_empty());
        self.last_closed_had_tabs.store(had_tabs, Ordering::SeqCst);
        if !self.quitting.load(Ordering::SeqCst) {
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
    /// last window with projects shows the Start screen instead of quitting.
    pub fn keep_running(&self) -> bool {
        !self.quitting.load(Ordering::SeqCst) && self.last_closed_had_tabs.load(Ordering::SeqCst)
    }

    fn save(&self) {
        let windows: Vec<WindowPlaces> = self
            .open
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, p)| !p.tabs.is_empty())
            .map(|(_, p)| p.clone())
            .collect();
        let result = serde_json::to_vec_pretty(&Saved::Windows { windows })
            .map_err(std::io::Error::other)
            .and_then(|bytes| {
                if let Some(dir) = self.saved.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                std::fs::write(&self.saved, bytes)
            });
        if let Err(err) = result {
            tracing::warn!("cannot remember the open projects: {err}");
        }
    }
}

/// The window's name in the Window menu and Mission Control: the project
/// shown and its machine.
fn title(places: &WindowPlaces) -> String {
    let shown = places
        .tabs
        .iter()
        .find(|t| Some(&t.host) == places.current.as_ref());
    let Some(Place { host, path }) = shown else {
        return "Sandhill".to_string();
    };
    let name = path.rsplit('/').find(|s| !s.is_empty()).unwrap_or(path);
    if host == sandhill_link::LOCAL {
        name.to_string()
    } else {
        format!("{name} — {host}")
    }
}

/// On macOS the page draws the title bar, under the window buttons, so the
/// panes can float on one background from the top of the window down.
/// The buttons sit in the page's 31px title bar: the red one 12px from the
/// left edge, centred on the capsules beside them, as in the design
/// (docs/design/ui/).
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

/// Where macOS puts the close button, in points. Measured on a Mac (the Mac
/// check prints it): x 11 puts the red light at 12-24pt, and the lights'
/// centre lands 2pt below y, so y 16.5 centres them on the capsules, which
/// are 25pt high from 6pt down: an 18.5pt line.
#[cfg(target_os = "macos")]
const TRAFFIC_LIGHTS_X: f64 = 11.0;
#[cfg(target_os = "macos")]
const TRAFFIC_LIGHTS_Y: f64 = 16.5;

#[cfg(not(target_os = "macos"))]
fn title_bar<'a, R: Runtime, M: Manager<R>>(
    builder: WebviewWindowBuilder<'a, R, M>,
) -> WebviewWindowBuilder<'a, R, M> {
    builder
}

/// What the calling window has open.
#[tauri::command]
pub fn window_context<R: Runtime>(
    window: WebviewWindow<R>,
    windows: State<'_, std::sync::Arc<Windows>>,
) -> WindowPlaces {
    windows.places(window.label()).unwrap_or_default()
}

/// Remembers what the calling window has open, and names the window after it.
#[tauri::command]
pub fn set_window_places<R: Runtime>(
    window: WebviewWindow<R>,
    windows: State<'_, std::sync::Arc<Windows>>,
    places: WindowPlaces,
    title: String,
) {
    let _ = window.set_title(&title);
    windows.set(window.label(), places);
}

/// A window kept itself open (unsaved changes), so Quit stops.
#[tauri::command]
pub fn cancel_quit(windows: State<'_, std::sync::Arc<Windows>>) {
    windows.cancel_quit();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(host: &str, path: &str) -> Place {
        Place {
            host: host.into(),
            path: path.into(),
        }
    }

    #[test]
    fn open_windows_are_remembered_except_while_quitting() {
        let dir = tempfile::tempdir().unwrap();
        let saved = dir.path().join("open-projects.json");
        let windows = Windows::new(saved.clone());
        let read = || -> Vec<WindowPlaces> {
            serde_json::from_str::<Saved>(&std::fs::read_to_string(&saved).unwrap())
                .unwrap()
                .windows()
        };
        let one = WindowPlaces {
            tabs: vec![place("local", "/w/a"), place("gpu01", "/w/b")],
            current: Some("gpu01".into()),
        };
        windows.set("window-1", WindowPlaces::default());
        windows.set("window-2", one.clone());
        assert_eq!(
            read(),
            std::slice::from_ref(&one),
            "an empty window isn't remembered"
        );
        assert_eq!(title(&one), "b — gpu01");

        windows.closed("window-1");
        assert!(!windows.keep_running(), "the Start screen closed: quit");
        windows.quitting.store(true, Ordering::SeqCst);
        windows.closed("window-2");
        assert_eq!(read(), [one], "quitting keeps the list for next time");
        assert!(!windows.keep_running());
    }

    #[test]
    fn closing_the_last_project_window_shows_the_start_screen() {
        let dir = tempfile::tempdir().unwrap();
        let windows = Windows::new(dir.path().join("open-projects.json"));
        windows.set(
            "window-1",
            WindowPlaces {
                tabs: vec![place("local", "/w/a")],
                current: Some("local".into()),
            },
        );
        windows.closed("window-1");
        assert!(windows.keep_running());
    }

    #[test]
    fn projects_from_before_machine_tabs_open_in_one_window() {
        let old = r#"[["local", "/w/a"], ["gpu01", "/w/b"], ["local", "/w/c"]]"#;
        let windows = serde_json::from_str::<Saved>(old).unwrap().windows();
        assert_eq!(
            windows,
            [WindowPlaces {
                tabs: vec![place("gpu01", "/w/b"), place("local", "/w/c")],
                current: Some("local".into()),
            }]
        );
    }
}

/// Opens a web link from the UI, like one in an agent's terminal, in the
/// default browser.
#[tauri::command]
pub fn open_link(url: String) -> Result<(), String> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(format!("not a web link: {url}"));
    }
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let mut child = std::process::Command::new(opener)
        .arg(&url)
        .spawn()
        .map_err(|err| format!("cannot open {url}: {err}"))?;
    std::thread::spawn(move || child.wait());
    Ok(())
}
