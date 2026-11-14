//! The terminal engine with a real ConPTY on Windows: the foreground
//! process, and closing the terminal ending the whole process tree.

#![cfg(all(windows, feature = "ghostty"))]

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_cn::terminal::{
    Engine, EngineOptions, FrameHandle, FrameSink, FrameSource, LocalPty, LocalTerminalOptions,
    ShellIntegration, StartOptions, TerminalSnapshot,
};

fn wait_for(
    sink: &FrameSink,
    handle: &dyn FrameHandle,
    done: impl Fn(&TerminalSnapshot) -> bool,
) -> Arc<TerminalSnapshot> {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut last = None;
    while Instant::now() < deadline {
        if let Some(snapshot) = sink.take() {
            handle.request_frame();
            if done(&snapshot) {
                return snapshot;
            }
            last = Some(snapshot);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!(
        "timed out; last frame:\n{}",
        last.map_or_else(String::new, |s| s.frame.text())
    );
}

fn alive(pid: u32) -> bool {
    let out = std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH"])
        .output()
        .expect("tasklist runs");
    String::from_utf8_lossy(&out.stdout).contains(&pid.to_string())
}

#[test]
fn closing_the_terminal_ends_the_program_and_what_it_started() {
    // cmd starts ping, a grandchild of the terminal; ping prints a line a
    // second, and output is what makes the engine look at the foreground.
    let options = LocalTerminalOptions::default()
        .with_program("cmd.exe", ["/d", "/c", "ping -n 1000 127.0.0.1"])
        .with_shell_integration(ShellIntegration::None);
    let (sink, _wake) = FrameSink::new();
    let handle = Box::new(Engine::new(
        LocalPty::new(options),
        EngineOptions::default(),
    ))
    .start(sink.clone(), StartOptions::default())
    .unwrap();
    let shown = wait_for(&sink, handle.as_ref(), |s| {
        s.foreground
            .as_ref()
            .is_some_and(|p| p.name().eq_ignore_ascii_case("ping.exe"))
    });
    let ping = shown.foreground.as_ref().unwrap();
    assert!(!ping.is_shell(), "{ping:?}");
    let pid = ping.pid();
    assert!(alive(pid));

    handle.close();
    let deadline = Instant::now() + Duration::from_secs(10);
    while alive(pid) {
        assert!(Instant::now() < deadline, "ping outlived the terminal");
        std::thread::sleep(Duration::from_millis(50));
    }
}
