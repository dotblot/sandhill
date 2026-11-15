//! A terminal on libghostty-vt, the terminal emulation library from
//! [Ghostty](https://ghostty.org), behind the `ghostty` feature.
//!
//! The pieces:
//!
//! - [`TerminalState`] is the entity: it holds the latest [`TerminalFrame`],
//!   the program's [`TerminalStatus`], the selection and the focus, and it
//!   sends [`TerminalInput`] to its source.
//! - [`Terminal`] is the element that paints a state and routes keys, the
//!   pointer and the input method to it.
//! - A [`FrameSource`] produces frames. [`Engine`] runs a [`ByteSource`]
//!   through a Ghostty terminal on an owner thread; [`LocalPty`] (feature
//!   desktop targets) is the byte source for a local program; [`StreamSource`]
//!   is one the application feeds itself, such as a remote session or its
//!   own pty; [`FixtureSource`] parses canned bytes for tests and galleries.
//! - [`crate::shell`] resolves the user's shell, its login command and
//!   the environment the way Ghostty does, for applications that start the
//!   program themselves.
//!
//! # Use it
//!
//! Enable the `ghostty` feature, with `cargo add gpui-cn --features ghostty`
//! or in `Cargo.toml`:
//!
//! ```toml
//! gpui-cn = { version = "0.5", features = ["ghostty", "jetbrains-mono"] }
//! ```
//!
//! Building it needs Zig 0.16 on `PATH`; the first build downloads
//! Ghostty's pinned source. Whether a terminal runs a local shell or another
//! source, and whether idle terminals park, are runtime choices.
//! After `gpui_cn::init`, a view holds a [`TerminalState`] and renders a
//! [`Terminal`] for it:
//!
//! ```no_run
//! # #[cfg(not(any(target_os = "ios", target_os = "android")))]
//! # mod example {
//! use gpui_cn::terminal::{LocalTerminalOptions, Terminal, TerminalConfig, TerminalState};
//! use gpui_kit::*;
//!
//! struct Shell {
//!     terminal: Entity<TerminalState>,
//! }
//!
//! impl Render for Shell {
//!     fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
//!         div().size_full().child(Terminal::new("shell", &self.terminal))
//!     }
//! }
//!
//! pub fn main() {
//!     gpui_kit::application()
//!         .with_assets(gpui_kit::assets::Assets)
//!         .run(|cx| {
//!             gpui_kit::init(cx);
//!             gpui_cn::init(cx);
//!             gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
//!                 cx.new(|cx| {
//!                     // The user's login shell, picked as Ghostty picks it.
//!                     let terminal = cx.new(|cx| {
//!                         TerminalState::local(
//!                             LocalTerminalOptions::default(),
//!                             TerminalConfig::default(),
//!                             window,
//!                             cx,
//!                         )
//!                     });
//!                     cx.observe(&terminal, |_, _, cx| cx.notify()).detach();
//!                     Shell { terminal }
//!                 })
//!             })
//!             .expect("failed to open window");
//!         });
//! }
//! # }
//! # fn main() {}
//! ```
//!
//! A remote session or an application's own pty feeds a [`StreamSource`]
//! instead of a local pty:
//!
//! ```no_run
//! # use gpui_cn::terminal::*;
//! # fn remote(window: &mut gpui_kit::Window, cx: &mut gpui_kit::Context<TerminalState>) -> TerminalState {
//! let (source, peer) = StreamSource::new();
//! peer.output(b"$ "); // the program's output; input arrives on peer.events()
//! TerminalState::new(Engine::new(source, EngineOptions::default()), TerminalConfig::default(), window, cx)
//! # }
//! ```
//!
//! A host that keeps the terminal elsewhere, such as a server that sends
//! the rows it wants drawn, implements [`FrameSource`] and publishes
//! frames it builds itself to the [`FrameSink`]. The builders on
//! [`TerminalFrame`], [`frame::TerminalRow`], [`frame::Style`],
//! [`frame::CellSpan`], [`frame::StyleRun`], [`frame::Cursor`] and
//! [`TerminalSnapshot`] make them. The [`FrameHandle`] it returns gets the
//! input, the measured grid and the request for the next frame; its `copy`
//! answers a selection that reaches scrollback the frame does not carry.
//! `tests/terminal_hosted.rs` is a small example.
//!
//! A local pty reports the process in the terminal's foreground, such as
//! the shell or an editor it started: [`TerminalState::foreground`] reads
//! it, [`TerminalEvent::ForegroundChanged`] reports a change, and
//! [`TerminalState::needs_confirm_close`] says whether closing would end a
//! program other than the shell. The engine looks after output only, so a
//! quiet terminal costs nothing.
//!
//! # Key bindings
//!
//! `gpui_cn::init` binds no terminal keys. A focused terminal takes the
//! keys a program needs on its own, without a key binding: every key it
//! can encode goes to the program, Tab and Shift-Tab too, which
//! `gpui-base`'s `Root` would otherwise use to move focus, and Ctrl-C,
//! which `Root` binds to copy outside macOS. Escape drops a selection
//! first, when there is one. Cmd-C on macOS copies the selection through
//! `Root`'s copy key.
//!
//! The rest are [`actions`] an application binds in the [`KEY_CONTEXT`]
//! key context. [`default_key_bindings`] holds Ghostty's keys for them:
//! copy and paste (Cmd-C and Cmd-V on macOS, Ctrl-Shift-C and Ctrl-Shift-V
//! elsewhere), select all, clear, Shift with Page Up, Page Down, Home,
//! End, Up and Down to scroll, and the font zoom: Cmd-= or Cmd-+ in, Cmd--
//! out and Cmd-0 back (Ctrl elsewhere), one point a step. An application
//! installs them, all or some, or binds its own:
//!
//! ```no_run
//! use gpui_cn::terminal::{KEY_CONTEXT, actions, default_key_bindings};
//! use gpui_kit::{App, KeyBinding, NoAction};
//!
//! fn keys(cx: &mut App) {
//!     cx.bind_keys(default_key_bindings());
//!     cx.bind_keys([
//!         // Zoom with Cmd-Up instead of Cmd-=.
//!         KeyBinding::new("cmd-up", actions::IncreaseFontSize, Some(KEY_CONTEXT)),
//!         KeyBinding::new("cmd-=", NoAction, Some(KEY_CONTEXT)),
//!     ]);
//! }
//! ```
//!
//! A binding an application adds in [`KEY_CONTEXT`] wins over a key the
//! terminal would take, Tab included. The bindings are per context, not
//! per terminal: every terminal in the application has them, and only
//! the focused one acts on a key.
//!
//! Tabs, splits, pane zoom, a leader key and context menus are the
//! application's to build; the gallery's Terminal story shows one way.
//!
//! The grid uses the theme's code font and code font size unless the
//! [`TerminalAppearance`] names others, so the `jetbrains-mono` feature
//! gives it JetBrains Mono. The colors are Ghostty's defaults;
//! [`TerminalColors::from_ghostty_theme`] reads a Ghostty theme file.
//!
//! The engine never blocks the UI: frames arrive through a latest-wins slot
//! and an async wake, input goes through a bounded queue that Ctrl+C jumps,
//! a hidden terminal keeps parsing but publishes nothing, and synchronized
//! output (mode 2026) holds frames for at most one second. The engine never
//! waits on the program either: a write the pty cannot take yet resumes
//! when it has room, and at most a megabyte of output waits for the parser
//! before the pty holds the program back, so a large paste into a program
//! that is busy writing cannot deadlock.
//!
//! Derived from Herdr (Apache-2.0); the block, decoration and element
//! structure are adapted from Muxy v2 (MIT).

mod appearance;
mod block;
mod colors;
mod core;
mod cursor;
mod decoration;
mod element;
mod engine;
pub mod frame;
mod geometry;
mod ime;
pub mod input;
mod keys;
mod links;
mod options;
pub mod park;
#[cfg(all(unix, not(any(target_os = "ios", target_os = "android"))))]
mod process;
#[cfg(windows)]
mod process_windows;
#[cfg(not(any(target_os = "ios", target_os = "android")))]
mod pty;
#[cfg(not(any(target_os = "ios", target_os = "android")))]
mod pty_io;
mod selection;
mod source;
mod state;
mod stream;
mod theme;

pub use appearance::TerminalAppearance;
pub use element::{KEY_CONTEXT, Terminal};
pub use engine::{
    ByteHandle, ByteSink, ByteSource, Engine, ExitStatus, ForegroundProcess, InputRejected,
    TerminalSnapshot, TerminalStatus,
};
pub use frame::{RowId, TerminalColors, TerminalFrame, Viewport};
pub use geometry::Geometry;
pub use input::TerminalInput;
pub use links::FileLink;
pub use options::{EngineOptions, LocalTerminalOptions, ShellIntegration, WorkingDirectory};
#[cfg(not(any(target_os = "ios", target_os = "android")))]
pub use pty::LocalPty;
pub use source::{FixtureSource, FrameHandle, FrameSink, FrameSource, StartOptions};
pub use state::{TerminalConfig, TerminalEvent, TerminalState};
pub use stream::{StreamEvent, StreamPeer, StreamSource};
pub use theme::ThemeError;

/// Actions dispatched in the [`KEY_CONTEXT`] key context.
pub mod actions {
    gpui_kit::actions!(
        terminal,
        [
            /// Copy the selection.
            Copy,
            /// Paste the clipboard.
            Paste,
            /// Select the viewport.
            SelectAll,
            /// Clear the screen through the shell.
            Clear,
            /// Scroll to the top of scrollback.
            ScrollToTop,
            /// Scroll to the bottom.
            ScrollToBottom,
            /// Scroll one page toward older output.
            ScrollPageUp,
            /// Scroll one page toward newer output.
            ScrollPageDown,
            /// Scroll one line toward older output.
            ScrollLineUp,
            /// Scroll one line toward newer output.
            ScrollLineDown,
            /// Zoom the font in one point, as Ghostty's `increase_font_size:1`.
            IncreaseFontSize,
            /// Zoom the font out one point, as Ghostty's `decrease_font_size:1`.
            DecreaseFontSize,
            /// Go back to the configured font size, as Ghostty's
            /// `reset_font_size`.
            ResetFontSize,
        ]
    );
}

/// Ghostty's keys for the terminal [`actions`], in the [`KEY_CONTEXT`] key
/// context, for an application to install with `cx.bind_keys(..)`. A
/// terminal works without them; see [Key bindings](self#key-bindings).
pub fn default_key_bindings() -> Vec<gpui_kit::KeyBinding> {
    use actions::{
        Clear, Copy, DecreaseFontSize, IncreaseFontSize, Paste, ResetFontSize, ScrollLineDown,
        ScrollLineUp, ScrollPageDown, ScrollPageUp, ScrollToBottom, ScrollToTop, SelectAll,
    };
    use gpui_kit::KeyBinding;
    let context = Some(KEY_CONTEXT);
    // Ghostty binds the font size to Cmd on macOS and Ctrl elsewhere, with
    // `+` beside `=` for keyboards that have a plus key.
    let [copy, paste, select_all, clear, larger, plus, smaller, reset] =
        if cfg!(target_os = "macos") {
            [
                "cmd-c", "cmd-v", "cmd-a", "cmd-k", "cmd-=", "cmd-+", "cmd--", "cmd-0",
            ]
        } else {
            [
                "ctrl-shift-c",
                "ctrl-shift-v",
                "ctrl-shift-a",
                "ctrl-shift-k",
                "ctrl-=",
                "ctrl-+",
                "ctrl--",
                "ctrl-0",
            ]
        };
    vec![
        KeyBinding::new(copy, Copy, context),
        KeyBinding::new(paste, Paste, context),
        KeyBinding::new(select_all, SelectAll, context),
        KeyBinding::new(clear, Clear, context),
        KeyBinding::new("shift-pageup", ScrollPageUp, context),
        KeyBinding::new("shift-pagedown", ScrollPageDown, context),
        KeyBinding::new("shift-home", ScrollToTop, context),
        KeyBinding::new("shift-end", ScrollToBottom, context),
        KeyBinding::new("shift-up", ScrollLineUp, context),
        KeyBinding::new("shift-down", ScrollLineDown, context),
        KeyBinding::new(larger, IncreaseFontSize, context),
        KeyBinding::new(plus, IncreaseFontSize, context),
        KeyBinding::new(smaller, DecreaseFontSize, context),
        KeyBinding::new(reset, ResetFontSize, context),
    ]
}
