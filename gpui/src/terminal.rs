//! The Terminal pane: a shell that sandhilld keeps running, drawn with
//! Ghostty's terminal engine (libghostty-vt) through gpui-cn's terminal
//! element.
//!
//! The daemon owns the program and its output. This view feeds the bytes
//! to a [`StreamSource`], which the Ghostty engine parses into a screen;
//! the keys, pastes and replies the engine encodes, and the grid sizes it
//! measures, go back to the daemon.

use base64::Engine as _;
use gpui_cn::terminal::{
    Engine, EngineOptions, StreamEvent, StreamPeer, StreamSource, Terminal, TerminalAppearance,
    TerminalColors, TerminalConfig, TerminalState,
};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use tokio::sync::mpsc::UnboundedSender;

use crate::look::{self, Palette};

/// The Tauri app's 13px, 2px larger; lines as tall as the font's own, so
/// box-drawing lines join up.
pub const FONT_SIZE: f32 = look::CODE;

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
    /// The appearance the terminal's colours were made for.
    dark: Option<bool>,
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
        TerminalView {
            id,
            peer,
            source: Some(source),
            state: None,
            window,
            focus: cx.focus_handle(),
            exited: None,
            dark: None,
            _events: events,
        }
    }

    fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(source) = self.source.take() else {
            return;
        };
        let p = *Palette::get(cx);
        self.dark = Some(p.dark);
        let config = TerminalConfig::default()
            .with_colors(colors(&p))
            .with_appearance(
                TerminalAppearance::default()
                    .with_font_family(look::MONO)
                    .with_font_size(px(FONT_SIZE))
                    .with_padding(px(6.)),
            );
        let state = cx.new(|cx| {
            TerminalState::new(
                Engine::new(source, EngineOptions::default()),
                config,
                window,
                cx,
            )
        });
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        self.state = Some(state);
        cx.notify();
    }

    /// Output from the program, base64 as the daemon sends it.
    pub fn feed(&mut self, data: &str, _cx: &mut Context<Self>) {
        if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(data) {
            self.peer.output(&bytes);
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
        let Some(state) = self.state.clone() else {
            return div()
                .id("terminal")
                .track_focus(&self.focus)
                .size_full()
                .bg(p.term)
                .into_any_element();
        };
        // The theme changed: the terminal follows.
        if self.dark != Some(p.dark) {
            self.dark = Some(p.dark);
            state.update(cx, |s, cx| s.set_colors(colors(&p), cx));
        }
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
                        .text_size(px(look::SMALL))
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
    fn turns_palette_colours_into_bytes() {
        let c: Hsla = rgb(0x1e8f4c).into();
        let out = to_rgb(c);
        assert_eq!((out.0, out.1, out.2), (0x1e, 0x8f, 0x4c));
    }
}
