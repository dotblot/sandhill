//! The I/O of a local pty, without blocking the engine.
//!
//! The engine never waits on the program. Input is written as far as the
//! pty takes it and the rest is kept, in order, until the pty has room
//! again; output is read only while the engine's read queue has room, so a
//! program that floods the terminal is held back by the pty, not by memory.
//! Neither direction waits on the other, so a large paste into a program
//! that is busy writing cannot deadlock.
//!
//! On Unix one thread owns both directions: it polls the pty for output
//! and, while a write is short, for room to write, and wakes the engine
//! when the pty can take input again. Input itself is written by the
//! engine with non-blocking writes. On Windows, where a ConPTY pipe has no
//! non-blocking mode, a writer thread takes input from a bounded buffer.

use std::io;

use crate::terminal::engine::ByteSink;

/// How much output one read takes.
const READ_BUFFER: usize = 64 * 1024;

#[cfg(unix)]
pub(crate) use unix::PollIo;
#[cfg(not(unix))]
pub(crate) use writer::WriterThread;

#[cfg(unix)]
mod unix {
    use std::io;
    use std::os::fd::{AsRawFd as _, FromRawFd as _, OwnedFd, RawFd};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::{READ_BUFFER, read_into};
    use crate::terminal::engine::ByteSink;

    /// The pty master in non-blocking mode, with the thread that reads it.
    pub(crate) struct PollIo {
        /// The master, owned by the pty handle, which outlives this.
        master: RawFd,
        shared: Arc<Shared>,
    }

    struct Shared {
        /// A write was short; the thread watches for room to write.
        want_writable: AtomicBool,
        closed: AtomicBool,
        /// The write end of the pipe that wakes the thread.
        wake: OwnedFd,
    }

    impl Shared {
        fn poke(&self) {
            // A full pipe already holds a wake.
            // SAFETY: the buffer is one valid byte.
            unsafe { libc::write(self.wake.as_raw_fd(), [1u8].as_ptr().cast(), 1) };
        }
    }

    impl PollIo {
        /// Puts `master` in non-blocking mode and starts the thread that
        /// reads it into `sink`.
        pub(crate) fn start(master: RawFd, sink: ByteSink) -> io::Result<Self> {
            set_nonblocking(master)?;
            // The thread's own descriptor, so it never reads a closed or
            // reused one. It shares the non-blocking mode with `master`.
            // SAFETY: dup only creates a descriptor.
            let reader = cvt(unsafe { libc::dup(master) })?;
            // SAFETY: dup returned a new descriptor that nothing else owns.
            let reader = unsafe { OwnedFd::from_raw_fd(reader) };
            set_cloexec(reader.as_raw_fd())?;
            let (wake_rx, wake_tx) = pipe()?;
            let shared = Arc::new(Shared {
                want_writable: AtomicBool::new(false),
                closed: AtomicBool::new(false),
                wake: wake_tx,
            });
            let thread = shared.clone();
            std::thread::Builder::new()
                .name("gpui-cn-pty".to_owned())
                .spawn(move || run(&reader, &wake_rx, &sink, &thread))?;
            Ok(Self { master, shared })
        }

        /// Writes what the pty takes now. A short write has the thread
        /// watch for room and call [`ByteSink::writable`].
        pub(crate) fn write(&self, bytes: &[u8]) -> io::Result<usize> {
            if bytes.is_empty() {
                return Ok(0);
            }
            loop {
                // SAFETY: the buffer is valid for its length.
                let written =
                    unsafe { libc::write(self.master, bytes.as_ptr().cast(), bytes.len()) };
                if let Ok(written) = usize::try_from(written) {
                    if written < bytes.len() {
                        self.want_writable();
                    }
                    return Ok(written);
                }
                let error = io::Error::last_os_error();
                match error.kind() {
                    io::ErrorKind::Interrupted => {}
                    io::ErrorKind::WouldBlock => {
                        self.want_writable();
                        return Ok(0);
                    }
                    _ => return Err(error),
                }
            }
        }

        fn want_writable(&self) {
            self.shared.want_writable.store(true, Ordering::SeqCst);
            self.shared.poke();
        }

        /// Stops the thread.
        pub(crate) fn close(&self) {
            self.shared.closed.store(true, Ordering::SeqCst);
            self.shared.poke();
        }
    }

    impl Drop for PollIo {
        fn drop(&mut self) {
            self.close();
        }
    }

    fn run(reader: &OwnedFd, wake: &OwnedFd, sink: &ByteSink, shared: &Shared) {
        let mut buf = vec![0u8; READ_BUFFER];
        while !shared.closed.load(Ordering::SeqCst) {
            let want = shared.want_writable.load(Ordering::SeqCst);
            let mut fds = [
                libc::pollfd {
                    fd: reader.as_raw_fd(),
                    events: libc::POLLIN | if want { libc::POLLOUT } else { 0 },
                    revents: 0,
                },
                libc::pollfd {
                    fd: wake.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];
            // SAFETY: the array holds two valid pollfd entries.
            if unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) } < 0 {
                if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                break;
            }
            if fds[1].revents != 0 {
                drain(wake.as_raw_fd());
            }
            if shared.closed.load(Ordering::SeqCst) {
                break;
            }
            let events = fds[0].revents;
            if events & libc::POLLOUT != 0 && shared.want_writable.swap(false, Ordering::SeqCst) {
                sink.writable();
            }
            if events & libc::POLLNVAL != 0 {
                break;
            }
            if events & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0 {
                if !sink.wait_for_room() {
                    break;
                }
                match read_into(reader.as_raw_fd(), &mut buf) {
                    Ok(0) => break,
                    Ok(read) => {
                        if !sink.write(&buf[..read]) {
                            break;
                        }
                    }
                    Err(error)
                        if matches!(
                            error.kind(),
                            io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                        ) =>
                    {
                        // A hang-up with nothing left to read is the end.
                        if events & libc::POLLHUP != 0 {
                            break;
                        }
                    }
                    // EIO: the program and every holder of the pty are gone.
                    Err(_) => break,
                }
            }
        }
        sink.finished();
    }

    fn drain(fd: RawFd) {
        let mut buf = [0u8; 64];
        // SAFETY: the buffer is valid for its length; the pipe is
        // non-blocking, so this stops when it is empty.
        while unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) } > 0 {}
    }

    fn cvt(result: libc::c_int) -> io::Result<libc::c_int> {
        if result < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(result)
        }
    }

    fn set_nonblocking(fd: RawFd) -> io::Result<()> {
        // SAFETY: fcntl reads and sets the descriptor's flags.
        let flags = cvt(unsafe { libc::fcntl(fd, libc::F_GETFL) })?;
        cvt(unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) })?;
        Ok(())
    }

    fn set_cloexec(fd: RawFd) -> io::Result<()> {
        // SAFETY: fcntl reads and sets the descriptor's flags.
        let flags = cvt(unsafe { libc::fcntl(fd, libc::F_GETFD) })?;
        cvt(unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) })?;
        Ok(())
    }

    /// A non-blocking pipe, as (read end, write end).
    fn pipe() -> io::Result<(OwnedFd, OwnedFd)> {
        let mut fds = [0 as libc::c_int; 2];
        // SAFETY: the array holds the two descriptors pipe fills.
        cvt(unsafe { libc::pipe(fds.as_mut_ptr()) })?;
        // SAFETY: pipe returned two new descriptors that nothing else owns.
        let (rx, tx) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
        for fd in [&rx, &tx] {
            set_nonblocking(fd.as_raw_fd())?;
            set_cloexec(fd.as_raw_fd())?;
        }
        Ok((rx, tx))
    }
}

/// Reads what is there into `buf`.
#[cfg(unix)]
fn read_into(fd: std::os::fd::RawFd, buf: &mut [u8]) -> io::Result<usize> {
    // SAFETY: the buffer is valid for its length.
    let read = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
    usize::try_from(read).map_err(|_| io::Error::last_os_error())
}

/// Reads a blocking pty into `sink` on a thread of its own, holding back
/// while the engine's read queue is full.
#[cfg_attr(unix, allow(dead_code))]
pub(crate) fn spawn_reader(mut reader: Box<dyn io::Read + Send>, sink: ByteSink) -> io::Result<()> {
    std::thread::Builder::new()
        .name("gpui-cn-pty".to_owned())
        .spawn(move || {
            let mut buf = vec![0u8; READ_BUFFER];
            while sink.wait_for_room() {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => {
                        if !sink.write(&buf[..read]) {
                            break;
                        }
                    }
                }
            }
            sink.finished();
        })?;
    Ok(())
}

/// Input written on a thread of its own, for a pty whose writes block.
#[cfg_attr(unix, allow(dead_code))]
mod writer {
    use std::collections::VecDeque;
    use std::io::{self, Write};
    use std::sync::{Arc, Condvar, Mutex};

    /// The input that may wait for the writer thread.
    const BUFFER: usize = 64 * 1024;
    /// The most one blocking write takes.
    const CHUNK: usize = 8 * 1024;

    /// A bounded buffer of input and the thread that writes it out.
    pub(crate) struct WriterThread {
        shared: Arc<Shared>,
    }

    struct Shared {
        state: Mutex<State>,
        ready: Condvar,
    }

    #[derive(Default)]
    struct State {
        bytes: VecDeque<u8>,
        /// A write took less than it was given; `writable` follows room.
        short: bool,
        closed: bool,
        failed: bool,
    }

    impl Shared {
        fn lock(&self) -> std::sync::MutexGuard<'_, State> {
            self.state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        }
    }

    impl WriterThread {
        /// Starts the thread that writes to `writer`. `writable` runs on it
        /// when a short write's buffer has room again.
        pub(crate) fn start(
            mut writer: Box<dyn Write + Send>,
            writable: impl Fn() + Send + 'static,
        ) -> io::Result<Self> {
            let shared = Arc::new(Shared {
                state: Mutex::new(State::default()),
                ready: Condvar::new(),
            });
            let thread = shared.clone();
            std::thread::Builder::new()
                .name("gpui-cn-pty-writer".to_owned())
                .spawn(move || {
                    let mut chunk = Vec::with_capacity(CHUNK);
                    loop {
                        {
                            let mut state = thread.lock();
                            while state.bytes.is_empty() && !state.closed {
                                state = thread
                                    .ready
                                    .wait(state)
                                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                            }
                            if state.closed {
                                return;
                            }
                            let take = state.bytes.len().min(CHUNK);
                            chunk.clear();
                            chunk.extend(state.bytes.drain(..take));
                            if std::mem::take(&mut state.short) {
                                drop(state);
                                writable();
                            }
                        }
                        if writer
                            .write_all(&chunk)
                            .and_then(|()| writer.flush())
                            .is_err()
                        {
                            thread.lock().failed = true;
                            return;
                        }
                    }
                })?;
            Ok(Self { shared })
        }

        /// Takes as much of `bytes` as the buffer has room for.
        pub(crate) fn write(&self, bytes: &[u8]) -> io::Result<usize> {
            let mut state = self.shared.lock();
            if state.failed {
                return Err(io::Error::from(io::ErrorKind::BrokenPipe));
            }
            let take = bytes.len().min(BUFFER.saturating_sub(state.bytes.len()));
            state.bytes.extend(&bytes[..take]);
            if take < bytes.len() {
                state.short = true;
            }
            self.shared.ready.notify_one();
            Ok(take)
        }

        /// Stops the thread once its current write ends.
        pub(crate) fn close(&self) {
            self.shared.lock().closed = true;
            self.shared.ready.notify_one();
        }
    }

    impl Drop for WriterThread {
        fn drop(&mut self) {
            self.close();
        }
    }

    #[cfg(all(test, unix))]
    mod tests {
        use super::*;
        use std::io::Read as _;
        use std::os::fd::FromRawFd as _;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::{Duration, Instant};

        #[test]
        fn a_full_buffer_takes_part_and_calls_back_when_it_drains() {
            let mut fds = [0 as libc::c_int; 2];
            // SAFETY: the array holds the two descriptors pipe fills.
            assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
            // SAFETY: pipe returned two new descriptors that nothing owns.
            let (mut rx, tx) = unsafe {
                (
                    std::fs::File::from_raw_fd(fds[0]),
                    std::fs::File::from_raw_fd(fds[1]),
                )
            };
            let calls = Arc::new(AtomicUsize::new(0));
            let writer = WriterThread::start(Box::new(tx), {
                let calls = calls.clone();
                move || {
                    calls.fetch_add(1, Ordering::SeqCst);
                }
            })
            .unwrap();
            // Nothing reads the pipe, so its buffer and then ours fill.
            let input = vec![b'x'; 1 << 20];
            let mut taken = 0;
            let deadline = Instant::now() + Duration::from_secs(5);
            while taken < input.len() && Instant::now() < deadline {
                let took = writer.write(&input[taken..]).unwrap();
                if took == 0 {
                    break;
                }
                taken += took;
                std::thread::sleep(Duration::from_millis(5));
            }
            assert!(taken < input.len(), "a writer nobody reads fills up");
            // The thread reported room while it moved chunks into the pipe;
            // now it is blocked and the buffer is full.
            let before = calls.load(Ordering::SeqCst);

            // Reading frees room, which the thread reports.
            let mut out = vec![0u8; taken];
            let reader = std::thread::spawn(move || {
                rx.read_exact(&mut out).unwrap();
                out.len()
            });
            let deadline = Instant::now() + Duration::from_secs(5);
            while calls.load(Ordering::SeqCst) == before {
                assert!(Instant::now() < deadline, "no callback");
                std::thread::sleep(Duration::from_millis(5));
            }
            assert_eq!(reader.join().unwrap(), taken);
        }
    }
}
