//! Pseudo-terminals: a program attached to a terminal whose other end the
//! terminal host reads and writes.

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};

/// A running program and the controlling side of its terminal.
pub struct Pty {
    pub master: OwnedFd,
    pub child: Child,
}

/// Starts `command` with a new terminal of `cols` × `rows` as its stdin,
/// stdout and stderr. It leads a session of its own with that terminal as
/// its controlling terminal, as a shell in a terminal window does, so
/// Ctrl-C reaches it and it gets SIGHUP when the terminal closes.
pub fn spawn(command: &mut Command, cols: u16, rows: u16) -> io::Result<Pty> {
    let mut master: RawFd = -1;
    let mut slave: RawFd = -1;
    let mut size = winsize(cols, rows);
    // SAFETY: openpty writes the two descriptors; the name and termios
    // arguments may be null, and the window size outlives the call.
    let rc = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut::<libc::termios>() as _,
            &mut size as *mut libc::winsize as _,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: openpty succeeded, so both descriptors are open and ours.
    let (master, slave) = unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
    set_cloexec(master.as_raw_fd())?;
    command
        .stdin(Stdio::from(slave.try_clone()?))
        .stdout(Stdio::from(slave.try_clone()?))
        .stderr(Stdio::from(slave));
    // SAFETY: setsid and ioctl are async-signal-safe, as code between fork
    // and exec must be.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = command.spawn()?;
    Ok(Pty { master, child })
}

pub fn winsize(cols: u16, rows: u16) -> libc::winsize {
    libc::winsize {
        ws_row: rows.max(1),
        ws_col: cols.max(1),
        ws_xpixel: 0,
        ws_ypixel: 0,
    }
}

/// Sets the terminal's size. The kernel tells the program with SIGWINCH
/// when the size changed.
pub fn resize(master: RawFd, cols: u16, rows: u16) -> io::Result<()> {
    let size = winsize(cols, rows);
    // SAFETY: TIOCSWINSZ reads a winsize that lives across the call.
    if unsafe { libc::ioctl(master, libc::TIOCSWINSZ as _, &size) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// The process group the terminal is showing (the shell, or what it runs).
pub fn foreground_group(master: RawFd) -> Option<libc::pid_t> {
    // SAFETY: plain query on a descriptor we own.
    let pgrp = unsafe { libc::tcgetpgrp(master) };
    (pgrp > 0).then_some(pgrp)
}

pub fn set_nonblocking(fd: RawFd) -> io::Result<()> {
    // SAFETY: fcntl on a descriptor we own.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        if flags < 0 || libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

fn set_cloexec(fd: RawFd) -> io::Result<()> {
    // SAFETY: fcntl on a descriptor we own.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFD);
        if flags < 0 || libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}
