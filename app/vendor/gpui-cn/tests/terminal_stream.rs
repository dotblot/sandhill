//! A terminal driven by the application's own byte stream, as a remote
//! session or an application's own pty would drive it. The engine runs on
//! its own thread, so the test reads its frames from the sink, as the
//! pty test does.

#![cfg(feature = "ghostty")]

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_cn::terminal::input::KeyInput;
use gpui_cn::terminal::{
    Engine, EngineOptions, ExitStatus, FrameHandle, FrameSink, FrameSource, StartOptions,
    StreamEvent, StreamPeer, StreamSource, TerminalInput, TerminalSnapshot, TerminalStatus,
    Viewport,
};

/// Waits until a snapshot satisfies `done`, granting a frame after each.
fn wait_for(
    sink: &FrameSink,
    handle: &dyn FrameHandle,
    done: impl Fn(&TerminalSnapshot) -> bool,
) -> Arc<TerminalSnapshot> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut last = None;
    while Instant::now() < deadline {
        if let Some(snapshot) = sink.take() {
            handle.request_frame();
            if done(&snapshot) {
                return snapshot;
            }
            last = Some(snapshot);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!(
        "timed out; last frame:\n{}",
        last.map_or_else(String::new, |s| s.frame.text())
    );
}

/// Waits until the peer has seen `event`.
fn wait_event(peer: &StreamPeer, event: &StreamEvent) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        while let Ok(seen) = peer.events().try_recv() {
            if &seen == event {
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("timed out waiting for {event:?}");
}

#[test]
fn a_custom_stream_renders_output_gets_input_and_sizes_and_reports_exit() {
    let (source, peer) = StreamSource::new();
    // Output written before the terminal starts is kept.
    peer.output(b"remote$ ");
    let (sink, _wake) = FrameSink::new();
    let start = Viewport::new(60, 10, 8, 16).unwrap();
    let handle = Box::new(Engine::new(source, EngineOptions::default()))
        .start(sink.clone(), StartOptions::default().with_viewport(start))
        .unwrap();
    wait_event(&peer, &StreamEvent::Resize(start));

    peer.output(b"ls\r\nnotes.txt\r\n");
    let shown = wait_for(&sink, handle.as_ref(), |s| {
        s.frame.text().contains("notes.txt")
    });
    assert!(shown.frame.text().starts_with("remote$ ls\nnotes.txt"));
    assert_eq!(shown.status, TerminalStatus::Live);

    // Keys arrive encoded for the program.
    handle
        .input(TerminalInput::Key(KeyInput::new("a").with_text("a")))
        .unwrap();
    wait_event(&peer, &StreamEvent::Input(b"a".to_vec()));

    let bigger = Viewport::new(100, 30, 8, 16).unwrap();
    handle.resize(bigger);
    wait_event(&peer, &StreamEvent::Resize(bigger));
    let resized = wait_for(&sink, handle.as_ref(), |s| s.frame.viewport == bigger);
    assert_eq!(resized.frame.rows.len(), 30);

    peer.exit(ExitStatus::new(Some(3), None::<String>));
    let exited = wait_for(&sink, handle.as_ref(), |s| {
        matches!(s.status, TerminalStatus::Exited(_))
    });
    let TerminalStatus::Exited(status) = &exited.status else {
        unreachable!()
    };
    assert_eq!(status.code(), Some(3));

    handle.close();
    wait_event(&peer, &StreamEvent::Closed);
}

/// Starts an engine on a stream, takes its first frame and grants credit
/// for the next, as a painted terminal does.
fn started(viewport: Viewport) -> (StreamPeer, FrameSink, Box<dyn FrameHandle>) {
    let (source, peer) = StreamSource::new();
    let (sink, _wake) = FrameSink::new();
    let handle = Box::new(Engine::new(source, EngineOptions::default()))
        .start(
            sink.clone(),
            StartOptions::default().with_viewport(viewport),
        )
        .unwrap();
    wait_for(&sink, handle.as_ref(), |_| true);
    (peer, sink, handle)
}

/// Fails if the engine publishes a frame within `window`.
fn assert_no_frame(sink: &FrameSink, window: Duration, why: &str) {
    let deadline = Instant::now() + window;
    while Instant::now() < deadline {
        assert!(sink.take().is_none(), "{why}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Program output: `lines` numbered lines, the last one marked.
fn flood(lines: usize) -> Vec<u8> {
    let mut bytes = Vec::new();
    for line in 0..lines {
        bytes.extend_from_slice(format!("line {line} the quick brown fox\r\n").as_bytes());
    }
    bytes.extend_from_slice(b"LAST");
    bytes
}

#[test]
fn an_idle_terminal_builds_no_frames() {
    let (_peer, sink, handle) = started(Viewport::new(80, 24, 8, 16).unwrap());
    assert_no_frame(&sink, Duration::from_millis(400), "nothing changed");
    handle.close();
}

#[test]
fn a_hidden_terminal_drains_output_without_frames_and_shows_it_when_revealed() {
    let (peer, sink, handle) = started(Viewport::new(80, 24, 8, 16).unwrap());
    handle.set_visible(false);
    peer.output(&flood(50_000));
    assert_no_frame(
        &sink,
        Duration::from_millis(400),
        "a hidden terminal draws nothing",
    );
    handle.set_visible(true);
    let shown = wait_for(&sink, handle.as_ref(), |s| s.frame.text().contains("LAST"));
    assert!(
        shown.frame.text().contains("line 49999"),
        "the output was parsed"
    );
    handle.close();
}

#[test]
fn a_terminal_that_is_not_painted_builds_no_more_frames() {
    // The first frame is taken but never painted, so no credit comes back:
    // what a terminal on a page that is not shown, or in a window that is
    // minimized, sees.
    let (source, peer) = StreamSource::new();
    let (sink, _wake) = FrameSink::new();
    let handle = Box::new(Engine::new(source, EngineOptions::default()))
        .start(
            sink.clone(),
            StartOptions::default().with_viewport(Viewport::new(80, 24, 8, 16).unwrap()),
        )
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while sink.take().is_none() {
        assert!(Instant::now() < deadline, "no first frame");
        std::thread::sleep(Duration::from_millis(5));
    }
    peer.output(&flood(20_000));
    assert_no_frame(&sink, Duration::from_millis(400), "no credit, no frame");
    // A paint grants one frame, and it is current.
    handle.request_frame();
    let shown = wait_for(&sink, handle.as_ref(), |s| s.frame.text().contains("LAST"));
    assert!(shown.frame.text().contains("line 19999"));
    handle.close();
}

/// Read only program input, preserving its exact escape bytes and ordering.
/// Call after observing a new screen marker: the owner flushes replies before
/// publishing that frame, so this is a deterministic fence, not a quiet timer.
fn take_input(peer: &StreamPeer) -> Vec<u8> {
    let mut input = Vec::new();
    while let Ok(event) = peer.events().try_recv() {
        if let StreamEvent::Input(bytes) = event {
            input.extend(bytes);
        }
    }
    input
}

fn protocol_queries() -> Vec<(&'static [u8], Vec<u8>)> {
    vec![
        (b"\x1b[c", b"\x1b[?62;22c".to_vec()),
        (b"\x1b[6n", b"\x1b[1;1R".to_vec()),
        (b"\x1b]10;?\x1b\\", b"\x1b]10;rgb:ffff/ffff/ffff\x1b\\".to_vec()),
        (
            b"\x1b[>q",
            format!("\x1bP>|gpui-cn {}\x1b\\", env!("CARGO_PKG_VERSION")).into_bytes(),
        ),
    ]
}

#[test]
fn historical_queries_render_without_input_but_live_replies_keep_exact_bytes() {
    use gpui_cn::terminal::frame::Rgb;
    use gpui_cn::terminal::TerminalColors;

    let (source, peer) = StreamSource::new();
    // Exercise pre-start buffering as well as queries split over replay chunks.
    for (query, _) in protocol_queries() {
        for byte in query {
            peer.replay(&[*byte]);
        }
    }
    peer.replay(b"\x1b[32mHISTORY\x1b[0m");
    let (sink, _wake) = FrameSink::new();
    let handle = Box::new(Engine::new(source, EngineOptions::default()))
        .start(
            sink.clone(),
            StartOptions::default()
                .with_viewport(Viewport::new(80, 24, 8, 16).unwrap())
                .with_colors(TerminalColors::new(Rgb(255, 255, 255), Rgb(0, 0, 0))),
        )
        .unwrap();
    wait_for(&sink, handle.as_ref(), |s| s.frame.text().contains("HISTORY"));
    assert_eq!(take_input(&peer), b"", "historical queries cannot reach the shell");

    // The paired positive check proves those exact queries really do answer,
    // and that replay suppression is not a global disable or byte sanitizer.
    peer.output(b"\x1b[H");
    let mut expected = Vec::new();
    for (query, reply) in protocol_queries() {
        for byte in query {
            peer.output(&[*byte]);
        }
        expected.extend(reply);
    }
    peer.output(b"\r\nLIVE");
    wait_for(&sink, handle.as_ref(), |s| s.frame.text().contains("LIVE"));
    assert_eq!(take_input(&peer), expected);

    handle.input(TerminalInput::Paste("pasted".into())).unwrap();
    wait_event(&peer, &StreamEvent::Input(b"pasted".to_vec()));
    handle.close();
}

#[test]
fn a_query_started_in_replay_stays_historical_until_ground_then_live_queries_answer() {
    use gpui_cn::terminal::frame::Rgb;
    use gpui_cn::terminal::TerminalColors;

    for (query, expected) in protocol_queries() {
        for split in 1..query.len() {
            let (source, peer) = StreamSource::new();
            peer.replay(&query[..split]);
            // Preserve this boundary even if all output precedes engine start.
            peer.output(&query[split..]);
            // A fresh live query in the very same queued batch must answer.
            peer.output(query);
            peer.output(b"BOUNDARY");
            let (sink, _wake) = FrameSink::new();
            let handle = Box::new(Engine::new(source, EngineOptions::default()))
                .start(
                    sink.clone(),
                    StartOptions::default()
                        .with_viewport(Viewport::new(80, 24, 8, 16).unwrap())
                        .with_colors(TerminalColors::new(Rgb(255, 255, 255), Rgb(0, 0, 0))),
                )
                .unwrap();
            let shown = wait_for(&sink, handle.as_ref(), |s| {
                s.frame.text().contains("BOUNDARY")
            });
            assert!(shown.frame.text().starts_with("BOUNDARY"), "display is intact");
            assert_eq!(
                take_input(&peer), expected,
                "query {query:?} split at byte {split}: only the new live query replies",
            );
            handle.close();
        }
    }
}
