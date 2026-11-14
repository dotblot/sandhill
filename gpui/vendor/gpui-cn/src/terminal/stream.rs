//! A byte source an application drives itself: an ssh session, a
//! websocket to a remote shell, a pty the application owns, or a recorded
//! stream.
//!
//! [`StreamSource::new`] returns the source and its [`StreamPeer`]. The
//! source goes to [`Engine`](super::Engine) like any [`ByteSource`]; the
//! application keeps the peer, writes the program's output to it, reads
//! the terminal's input and grid sizes from it, and reports the exit.
//! It works on every target, including those with no local pty.
//!
//! ```no_run
//! use gpui_cn::terminal::{
//!     Engine, EngineOptions, ExitStatus, StreamEvent, StreamSource, TerminalConfig,
//!     TerminalState,
//! };
//! use gpui_kit::{AppContext as _, Context, Window};
//!
//! fn remote(window: &mut Window, cx: &mut Context<TerminalState>) -> TerminalState {
//!     let (source, peer) = StreamSource::new();
//!     cx.spawn(async move |_, _| {
//!         peer.output(b"connected\r\n$ ");
//!         while let Ok(event) = peer.events().recv().await {
//!             match event {
//!                 // Send the keys to the remote program.
//!                 StreamEvent::Input(bytes) => peer.output(&bytes),
//!                 // Tell the remote side the new size.
//!                 StreamEvent::Resize(_) => {}
//!                 StreamEvent::Closed => break,
//!                 _ => {}
//!             }
//!         }
//!         peer.exit(ExitStatus::new(Some(0), None::<String>));
//!     })
//!     .detach();
//!     let engine = Engine::new(source, EngineOptions::default());
//!     TerminalState::new(engine, TerminalConfig::default(), window, cx)
//! }
//! ```

use std::io;
use std::sync::{Arc, Mutex};

use gpui_kit::base::async_util;

use crate::terminal::engine::{ByteHandle, ByteSink, ByteSource, ExitStatus};
use crate::terminal::frame::Viewport;

/// What the terminal sends to the application's side of a
/// [`StreamSource`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum StreamEvent {
    /// Bytes for the program: keys, pastes and replies to its queries,
    /// already encoded for the terminal modes the program set.
    Input(Vec<u8>),
    /// The grid changed. The first event is the grid the terminal
    /// started with.
    Resize(Viewport),
    /// The terminal closed. Stop the program.
    Closed,
}

/// A [`ByteSource`] fed by its [`StreamPeer`].
#[derive(Debug)]
pub struct StreamSource {
    shared: Arc<Shared>,
}

/// The application's side of a [`StreamSource`]. Cheap to clone; every
/// clone feeds the same terminal.
#[derive(Clone, Debug)]
pub struct StreamPeer {
    shared: Arc<Shared>,
    events: async_util::Receiver<StreamEvent>,
}

#[derive(Debug)]
struct Shared {
    state: Mutex<State>,
    events: async_util::Sender<StreamEvent>,
}

/// Output written before the engine started waits here.
#[derive(Debug, Default)]
struct State {
    sink: Option<ByteSink>,
    pending: Vec<(Vec<u8>, bool)>,
    exit: Option<ExitStatus>,
}

impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl StreamSource {
    /// A source and the peer that feeds it.
    #[must_use]
    pub fn new() -> (Self, StreamPeer) {
        let (events, receiver) = async_util::unbounded();
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            events,
        });
        (
            Self {
                shared: shared.clone(),
            },
            StreamPeer {
                shared,
                events: receiver,
            },
        )
    }
}

impl StreamPeer {
    /// Delivers the program's output to the terminal. Output written
    /// before the terminal starts is kept until it does.
    pub fn output(&self, bytes: &[u8]) {
        self.deliver(bytes, false);
    }

    /// Reconstruct recorded output without sending query responses back to
    /// the program. Live output must use [`output`](Self::output), even when
    /// it follows replay before the engine has started.
    pub fn replay(&self, bytes: &[u8]) {
        self.deliver(bytes, true);
    }

    fn deliver(&self, bytes: &[u8], replay: bool) {
        let mut state = self.shared.lock();
        match &state.sink {
            Some(sink) => {
                if replay {
                    sink.replay(bytes);
                } else {
                    sink.write(bytes);
                }
            }
            None => match state.pending.last_mut() {
                Some((pending, kind)) if *kind == replay => pending.extend_from_slice(bytes),
                _ => state.pending.push((bytes.to_vec(), replay)),
            },
        }
    }

    /// Reports that the program ended. The terminal shows the status and
    /// emits [`TerminalEvent::Exited`](super::TerminalEvent::Exited).
    pub fn exit(&self, status: ExitStatus) {
        let mut state = self.shared.lock();
        state.exit = Some(status);
        if let Some(sink) = &state.sink {
            sink.finished();
        }
    }

    /// The terminal's input, grid sizes and close, in order. Await
    /// `recv()` on it, or poll `try_recv()`.
    pub fn events(&self) -> &async_util::Receiver<StreamEvent> {
        &self.events
    }
}

/// The engine's side of a started [`StreamSource`].
struct Handle {
    shared: Arc<Shared>,
}

impl ByteSource for StreamSource {
    fn start(
        self: Box<Self>,
        sink: ByteSink,
        viewport: Viewport,
    ) -> io::Result<Box<dyn ByteHandle>> {
        {
            let mut state = self.shared.lock();
            let pending = std::mem::take(&mut state.pending);
            for (bytes, replay) in pending {
                if replay {
                    sink.replay(&bytes);
                } else {
                    sink.write(&bytes);
                }
            }
            if state.exit.is_some() {
                sink.finished();
            }
            state.sink = Some(sink);
        }
        let _ = self.shared.events.try_send(StreamEvent::Resize(viewport));
        Ok(Box::new(Handle {
            shared: self.shared,
        }))
    }
}

impl ByteHandle for Handle {
    fn write(&self, bytes: &[u8]) -> io::Result<usize> {
        let _ = self
            .shared
            .events
            .try_send(StreamEvent::Input(bytes.to_vec()));
        Ok(bytes.len())
    }

    fn resize(&self, viewport: Viewport) -> io::Result<()> {
        let _ = self.shared.events.try_send(StreamEvent::Resize(viewport));
        Ok(())
    }

    fn try_wait(&self) -> io::Result<Option<ExitStatus>> {
        Ok(self.shared.lock().exit.clone())
    }

    fn close(&self) {
        let _ = self.shared.events.try_send(StreamEvent::Closed);
        self.shared.lock().sink = None;
    }
}
