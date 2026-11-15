//! A host that builds its own frames, as one that mirrors a remote
//! terminal does: it implements `FrameSource`, builds frames with the
//! public builders and publishes them to the sink.

#![cfg(feature = "ghostty")]

use std::{
    io,
    sync::{Arc, Mutex},
};

use gpui_cn::terminal::frame::{CellSpan, Color, Cursor, Rgb, Style, StyleRun, TerminalRow};
use gpui_cn::terminal::{
    FrameHandle, FrameSink, FrameSource, InputRejected, RowId, StartOptions, Terminal,
    TerminalConfig, TerminalFrame, TerminalInput, TerminalSnapshot, TerminalState, Viewport,
};
use gpui_kit::base::Root;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
    TestAppContext, Window, div, px, size,
};

/// Rows of `text`, each one run in `style`, with ids from `base`.
fn rows(lines: &[&str], style: &Style, base: u64) -> Vec<Arc<TerminalRow>> {
    lines
        .iter()
        .enumerate()
        .map(|(ix, line)| {
            let cells: Vec<CellSpan> = line
                .char_indices()
                .enumerate()
                .map(|(column, (start, ch))| {
                    CellSpan::new(column as u16, start..start + ch.len_utf8(), 1)
                })
                .collect();
            let runs = vec![StyleRun::new(
                0..line.len(),
                0..line.len() as u16,
                style.clone(),
            )];
            Arc::new(
                TerminalRow::new(RowId([base, ix as u64 + 1]), *line, cells, runs).with_revision(1),
            )
        })
        .collect()
}

#[derive(Default)]
struct Seen {
    viewports: Vec<Viewport>,
    inputs: Vec<TerminalInput>,
}

struct Hosted {
    seen: Arc<Mutex<Seen>>,
}

struct HostedHandle {
    seen: Arc<Mutex<Seen>>,
}

impl FrameSource for Hosted {
    fn start(
        self: Box<Self>,
        sink: FrameSink,
        options: StartOptions,
    ) -> io::Result<Box<dyn FrameHandle>> {
        let style = Style::new()
            .with_foreground(Color::Rgb(Rgb(0x80, 0xff, 0x80)))
            .with_bold(true);
        let viewport = Viewport::new(20, 2, 8, 16).expect("viewport");
        let frame = TerminalFrame::new(viewport, rows(&["remote$ ls", "file.txt"], &style, 7))
            .with_cursor(Cursor::new(10, 0))
            .with_colors(options.colors().clone());
        sink.publish(Arc::new(
            TerminalSnapshot::for_frame(Arc::new(frame)).with_title("remote: build"),
        ));
        Ok(Box::new(HostedHandle { seen: self.seen }))
    }
}

impl FrameHandle for HostedHandle {
    fn input(&self, input: TerminalInput) -> Result<(), InputRejected> {
        self.seen.lock().unwrap().inputs.push(input);
        Ok(())
    }

    fn resize(&self, viewport: Viewport) {
        self.seen.lock().unwrap().viewports.push(viewport);
    }

    fn set_visible(&self, _: bool) {}

    fn request_frame(&self) {}

    fn close(&self) {}
}

struct Harness {
    terminal: Entity<TerminalState>,
}

impl Render for Harness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(Terminal::new("terminal", &self.terminal))
    }
}

#[gpui_kit::test]
fn a_host_built_frame_reaches_the_terminal_and_the_grid_reaches_the_host(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
    });
    let seen = Arc::new(Mutex::new(Seen::default()));
    let terminal = Arc::new(Mutex::new(None));
    let handle = cx.open_window(size(px(640.), px(320.)), {
        let seen = seen.clone();
        let terminal = terminal.clone();
        move |window, cx| {
            let state = cx.new(|cx| {
                TerminalState::new(Hosted { seen }, TerminalConfig::default(), window, cx)
            });
            *terminal.lock().unwrap() = Some(state.clone());
            let harness = cx.new(|cx| {
                cx.observe(&state, |_, _, cx| cx.notify()).detach();
                Harness { terminal: state }
            });
            Root::new(harness, window, cx)
        }
    });
    let terminal = terminal.lock().unwrap().take().expect("the terminal");
    for _ in 0..3 {
        cx.run_until_parked();
        cx.update_window(handle.into(), |_, window, cx| window.render_frame(cx))
            .unwrap();
    }
    terminal.read_with(cx, |state, _| {
        assert_eq!(state.frame().text(), "remote$ ls\nfile.txt");
        assert_eq!(state.title().as_ref(), "remote: build");
    });
    // The measured grid, not the host's frame size, is what the host is told.
    let viewports = seen.lock().unwrap().viewports.clone();
    let last = viewports.last().expect("the host was told the grid");
    assert!(last.columns() > 20 && last.rows() > 2, "{last:?}");
}
