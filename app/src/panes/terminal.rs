//! The Terminal pane: a shell that sandhilld keeps running, drawn with
//! Ghostty's terminal engine (libghostty-vt) through gpui-cn's terminal
//! element.
//!
//! The daemon owns the program and its output. This view feeds the bytes
//! to a [`StreamSource`], which the Ghostty engine parses into a screen;
//! the keys, pastes and replies the engine encodes, and the grid sizes it
//! measures, go back to the daemon.

use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use gpui_cn::terminal::{
    Engine, EngineOptions, FileLink, StreamEvent, StreamPeer, StreamSource, Terminal,
    TerminalAppearance, TerminalColors, TerminalConfig, TerminalEvent, TerminalState,
};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use tokio::sync::mpsc::UnboundedSender;

use crate::daemon::Daemon;
use crate::look::{self, Palette};

struct FileLinks {
    daemon: Rc<Daemon>,
    project: String,
    start_cwd: String,
    cache: HashMap<String, (Instant, bool)>,
    pending: HashMap<String, FileLink>,
}

const LINK_CACHE_TTL: Duration = Duration::from_secs(3);
/// How often an undrawn terminal's screen is brought up to date.
const UNDRAWN_FRAME: Duration = Duration::from_millis(100);

/// The code font at the code size; lines as tall as the font's own, so
/// box-drawing lines join up.
fn appearance() -> TerminalAppearance {
    TerminalAppearance::default()
        .with_font_family(look::mono())
        .with_font_size(px(look::code()))
        .with_padding(px(6.))
}

/// Sets up gpui-cn, whose terminal element reads its theme, and the
/// terminal's keys (copy, paste, select all, scrolling, font size). Call
/// after `gpui_kit::init` and before `look::install`, so Sandhill's theme
/// is the one the rest of the app draws with.
pub fn init(cx: &mut App) {
    gpui_cn::init(cx);
    cx.bind_keys(gpui_cn::terminal::default_key_bindings());
}

pub struct TerminalView {
    pub id: String,
    /// The program's side of the engine: output goes in here.
    peer: StreamPeer,
    /// The engine's side, until the terminal starts.
    source: Option<StreamSource>,
    state: Option<Entity<TerminalState>>,
    window: AnyWindowHandle,
    /// What callers focus; the terminal takes the focus over when drawn.
    focus: FocusHandle,
    exited: Option<String>,
    /// Drawn since the engine's last frame arrived; an undrawn terminal
    /// (an agent's behind its chat) is read through `screen_text`, so it is
    /// given each next frame itself, at most every `UNDRAWN_FRAME`.
    drawn: bool,
    granting: bool,
    file_links: Option<FileLinks>,
    _theme: Subscription,
    _typography: Subscription,
    _events: Task<()>,
}

impl TerminalView {
    pub fn new(
        id: String,
        input: UnboundedSender<String>,
        resize: Box<dyn Fn(u16, u16)>,
        window: AnyWindowHandle,
        cx: &mut Context<Self>,
    ) -> Self {
        let (source, peer) = StreamSource::new();
        let events = peer.events().clone();
        // Keys and replies to the daemon in order, and the grid's size.
        let events = cx.spawn(async move |_, _| {
            while let Ok(event) = events.recv().await {
                match event {
                    StreamEvent::Input(bytes) => {
                        let _ = input.send(String::from_utf8_lossy(&bytes).into_owned());
                    }
                    StreamEvent::Resize(viewport) => resize(viewport.columns(), viewport.rows()),
                    _ => {}
                }
            }
        });
        // The engine starts once the window can be reached, so a terminal
        // that isn't shown (an agent's) still keeps its screen.
        let this = cx.weak_entity();
        cx.defer(move |cx| {
            let _ = window.update(cx, |_, window, cx| {
                this.update(cx, |view, cx| view.start(window, cx)).ok();
            });
        });
        let theme = cx.observe_global::<Palette>(|view, cx| {
            if let Some(state) = &view.state {
                let p = *Palette::get(cx);
                state.update(cx, |state, cx| state.set_colors(colors(&p), cx));
            }
            cx.notify();
        });
        let typography = cx.observe_global::<look::Typography>(|view, cx| {
            if let Some(state) = &view.state {
                state.update(cx, |state, cx| state.set_appearance(appearance(), cx));
            }
        });
        TerminalView {
            id,
            peer,
            source: Some(source),
            state: None,
            window,
            focus: cx.focus_handle(),
            exited: None,
            drawn: false,
            granting: false,
            _theme: theme,
            _typography: typography,
            file_links: None,
            _events: events,
        }
    }

    fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(source) = self.source.take() else {
            return;
        };
        let p = *Palette::get(cx);
        let config = TerminalConfig::default()
            .with_colors(colors(&p))
            .with_appearance(appearance());
        let state = cx.new(|cx| {
            TerminalState::new(
                Engine::new(source, EngineOptions::default()),
                config,
                window,
                cx,
            )
        });
        cx.observe(&state, |view, state, cx| {
            if !std::mem::take(&mut view.drawn) && !view.granting {
                view.granting = true;
                let state = state.downgrade();
                cx.spawn(async move |view, cx| {
                    cx.background_executor().timer(UNDRAWN_FRAME).await;
                    let _ = view.update(cx, |view, cx| {
                        view.granting = false;
                        if !view.drawn
                            && let Some(state) = state.upgrade()
                        {
                            state.read(cx).frame_read();
                        }
                    });
                })
                .detach();
            }
            cx.notify()
        })
        .detach();
        cx.subscribe(
            &state,
            |view, state, event: &TerminalEvent, cx| match event {
                TerminalEvent::FileLinkHovered(file) => {
                    view.check_file_link(file.clone(), &state, cx)
                }
                TerminalEvent::FileLinkOpened(file) => {
                    if let Some(file) = view.resolve_link(file, cx) {
                        cx.emit(file);
                    }
                }
                _ => {}
            },
        )
        .detach();
        if self.file_links.is_some() {
            state.update(cx, |state, cx| state.enable_file_links(cx));
        }
        self.state = Some(state);
        cx.notify();
    }

    /// Output from the program, base64 as the daemon sends it. Historical
    /// output restores the screen but cannot write replies into a live shell.
    pub fn feed(&mut self, data: &str, replay: bool, _cx: &mut Context<Self>) {
        if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(data) {
            if replay {
                self.peer.replay(&bytes);
            } else {
                self.peer.output(&bytes);
            }
        }
    }

    /// The program started again in this terminal.
    pub fn revive(&mut self, cx: &mut Context<Self>) {
        self.exited = None;
        cx.notify();
    }

    pub fn exited(&mut self, how: String, cx: &mut Context<Self>) {
        self.exited = Some(how);
        cx.notify();
    }

    /// The screen's text, one line per row, as the program last drew it.
    pub fn screen_text(&self, cx: &App) -> String {
        let Some(state) = &self.state else {
            return String::new();
        };
        state
            .read(cx)
            .frame()
            .text()
            .lines()
            .map(str::trim_end)
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    /// Configure daemon-only file validation for this project's terminal.
    pub(crate) fn set_file_links(
        &mut self,
        daemon: Rc<Daemon>,
        project: String,
        start_cwd: String,
        cx: &mut Context<Self>,
    ) {
        self.file_links = Some(FileLinks {
            daemon,
            project,
            start_cwd,
            cache: HashMap::new(),
            pending: HashMap::new(),
        });
        if let Some(state) = &self.state {
            state.update(cx, |state, cx| state.enable_file_links(cx));
        }
    }

    fn resolve_link(&self, file: &FileLink, cx: &App) -> Option<FileLink> {
        let links = self.file_links.as_ref()?;
        let cwd = self
            .state
            .as_ref()
            .and_then(|state| state.read(cx).cwd())
            .and_then(|cwd| cwd.to_str())
            .unwrap_or(&links.start_cwd);
        Some(FileLink {
            path: project_file(&file.path, cwd, &links.project)?,
            line: file.line,
            column: file.column,
        })
    }

    fn check_file_link(
        &mut self,
        candidate: FileLink,
        state: &Entity<TerminalState>,
        cx: &mut Context<Self>,
    ) {
        let Some(resolved) = self.resolve_link(&candidate, cx) else {
            state.update(cx, |state, cx| {
                state.confirm_file_link(&candidate, false, cx)
            });
            return;
        };
        let links = self.file_links.as_mut().unwrap();
        links
            .cache
            .retain(|_, (when, _)| when.elapsed() < LINK_CACHE_TTL);
        if let Some((_, exists)) = links.cache.get(&resolved.path) {
            state.update(cx, |state, cx| {
                state.confirm_file_link(&candidate, *exists, cx)
            });
            return;
        }
        if links
            .pending
            .insert(resolved.path.clone(), candidate)
            .is_some()
        {
            return;
        }
        let daemon = links.daemon.clone();
        let path = resolved.path.clone();
        let stat = daemon.call::<Vec<sandhill_proto::FileStat>>(sandhill_proto::Call::StatFiles {
            paths: vec![path.clone()],
        });
        let state = state.downgrade();
        cx.spawn(async move |this, cx| {
            let exists = stat.await.ok().is_some_and(|stats| {
                stats
                    .first()
                    .is_some_and(|stat| stat.path == path && stat.mtime_ms.is_some())
            });
            // StatFiles also includes directories. Confirm a regular file using
            // the daemon's directory metadata, never the Mac's filesystem.
            let exists = if exists {
                let parent = crate::panes::files::parent_of(&path);
                daemon
                    .call::<Vec<sandhill_proto::DirEntry>>(sandhill_proto::Call::ListDir {
                        path: parent,
                        sizes: false,
                        marks: false,
                    })
                    .await
                    .ok()
                    .is_some_and(|entries| {
                        entries.iter().any(|entry| {
                            entry.name == crate::panes::files::base_name(&path)
                                && entry.kind == sandhill_proto::EntryKind::File
                        })
                    })
            } else {
                false
            };
            this.update(cx, |view, cx| {
                let Some(links) = view.file_links.as_mut() else {
                    return;
                };
                if !Rc::ptr_eq(&daemon, &links.daemon) {
                    return;
                }
                let Some(candidate) = links.pending.remove(&path) else {
                    return;
                };
                if links.cache.len() >= 256 {
                    links.cache.clear();
                }
                links.cache.insert(path.clone(), (Instant::now(), exists));
                if view
                    .resolve_link(&candidate, cx)
                    .is_some_and(|file| file.path == path)
                {
                    state
                        .update(cx, |state, cx| {
                            state.confirm_file_link(&candidate, exists, cx)
                        })
                        .ok();
                }
            })
            .ok();
        })
        .detach();
    }
}

impl EventEmitter<FileLink> for TerminalView {}

/// Lexical POSIX resolution matches the daemon's local or remote filesystem.
fn project_file(path: &str, cwd: &str, project: &str) -> Option<String> {
    fn absolute(path: &str) -> Option<String> {
        if !path.starts_with('/') {
            return None;
        }
        let mut parts = Vec::new();
        for part in path.split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    parts.pop();
                }
                _ => parts.push(part),
            }
        }
        Some(format!("/{}", parts.join("/")))
    }
    if path.starts_with("~/") {
        return None;
    }
    let path = if path.starts_with('/') {
        absolute(path)?
    } else {
        absolute(&format!("{cwd}/{path}"))?
    };
    let project = absolute(project)?;
    let relative = path.strip_prefix(&project)?;
    (relative.starts_with('/') && relative.len() > 1 || project == "/" && path.len() > 1)
        .then_some(path)
}

/// The terminal's colours from Sandhill's palette: its background and
/// text, and its 16 ANSI colours over Ghostty's 256.
fn colors(p: &Palette) -> TerminalColors {
    let mut colors =
        TerminalColors::new(to_rgb(p.text), to_rgb(p.term)).with_cursor(to_rgb(p.accent));
    for (i, c) in p.ansi.iter().enumerate() {
        colors = colors.with_palette_entry(i as u8, to_rgb(*c));
    }
    colors
}

fn to_rgb(c: Hsla) -> gpui_cn::terminal::frame::Rgb {
    let c = c.to_rgb();
    let byte = |v: f32| (v.clamp(0., 1.) * 255.).round() as u8;
    gpui_cn::terminal::frame::Rgb(byte(c.r), byte(c.g), byte(c.b))
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = *Palette::get(cx);
        if self.state.is_none() && window.window_handle() == self.window {
            self.start(window, cx);
        }
        self.drawn = true;
        let Some(state) = self.state.clone() else {
            return div()
                .id("terminal")
                .track_focus(&self.focus)
                .size_full()
                .bg(p.term)
                .into_any_element();
        };
        // Focus given to this view goes on to the terminal.
        if self.focus.is_focused(window) {
            let inner = state.read(cx).focus_handle().clone();
            window.defer(cx, move |window, cx| window.focus(&inner, cx));
        }
        div()
            .id("terminal")
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .bg(p.term)
            .child(Terminal::new(
                SharedString::from(format!("term-{}", self.id)),
                &state,
            ))
            .when_some(self.exited.clone(), |d, how| {
                d.child(
                    div()
                        .absolute()
                        .bottom(px(6.))
                        .right(px(10.))
                        .px(px(6.))
                        .rounded(px(6.))
                        .bg(p.term)
                        .text_size(px(look::small()))
                        .text_color(p.muted)
                        .child(format!("[{how}]")),
                )
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::to_rgb;
    use gpui_kit::{Hsla, rgb};

    #[test]
    fn resolves_files_against_shell_cwd_with_project_boundaries() {
        use super::project_file;
        assert_eq!(
            project_file("a.py", "/p/src", "/p").as_deref(),
            Some("/p/src/a.py")
        );
        assert_eq!(
            project_file("../a.py", "/p/src", "/p").as_deref(),
            Some("/p/a.py")
        );
        assert!(project_file("../../secret", "/p/src", "/p").is_none());
        assert!(project_file("/project-other/a.py", "/p", "/project").is_none());
        assert!(project_file("/p", "/p", "/p").is_none());
        assert!(project_file("~/a.py", "/p", "/p").is_none());
        assert_eq!(project_file("a.py", "/p", "/").as_deref(), Some("/p/a.py"));
    }

    #[test]
    fn turns_palette_colours_into_bytes() {
        let c: Hsla = rgb(0x1e8f4c).into();
        let out = to_rgb(c);
        assert_eq!((out.0, out.1, out.2), (0x1e, 0x8f, 0x4c));
    }
}
