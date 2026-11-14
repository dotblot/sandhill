//! UI integration tests for `Terminal`: real input through a headless
//! window, with a `FixtureSource` that records what reaches the program.

#![cfg(feature = "ghostty")]

use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
};

use gpui_cn::ScrollArea;
use gpui_cn::terminal::input::KeyAction;
use gpui_cn::terminal::{
    ExitStatus, FixtureSource, Terminal, TerminalConfig, TerminalEvent, TerminalInput,
    TerminalSnapshot, TerminalState, TerminalStatus,
};
use gpui_kit::base::{Root, TestSupportExt as _};
use gpui_kit::{
    AppContext as _, Context, ElementId, Entity, EntityInputHandler as _, Focusable as _,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, ScrollDelta, Styled as _,
    TestAppContext, Window, WindowHandle, div, point, px, size, test::TestWindowExt as _,
};

const SCREEN: &str = "hello world\r\nsecond line";

struct Harness {
    terminal: Entity<TerminalState>,
    in_page: bool,
}

impl Render for Harness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let terminal = Terminal::new("terminal", &self.terminal);
        if !self.in_page {
            return div().size_full().child(terminal).into_any_element();
        }
        // A page taller than the window, with the terminal in it, as the
        // gallery shows one.
        ScrollArea::new("page")
            .size_full()
            .child(div().id("above").test_support().h(px(120.)).w_full())
            .child(div().h(px(160.)).w_full().child(terminal))
            .child(div().h(px(1000.)).w_full())
            .into_any_element()
    }
}

struct Setup {
    handle: WindowHandle<Root>,
    terminal: Entity<TerminalState>,
    inputs: Arc<Mutex<Vec<TerminalInput>>>,
    events: Rc<RefCell<Vec<TerminalEvent>>>,
}

/// A terminal in an application that installs the default key bindings.
fn setup(cx: &mut TestAppContext) -> Setup {
    setup_with(cx, false, true)
}

/// A terminal in an application that binds no terminal keys.
fn bare(cx: &mut TestAppContext) -> Setup {
    setup_with(cx, false, false)
}

fn setup_with(cx: &mut TestAppContext, in_page: bool, defaults: bool) -> Setup {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
        if defaults {
            cx.bind_keys(gpui_cn::terminal::default_key_bindings());
        }
    });
    let source = FixtureSource::new(SCREEN);
    let inputs = source.inputs();
    let source = Rc::new(RefCell::new(Some(source)));
    let terminal = Rc::new(RefCell::new(None));
    let handle = cx.open_window(size(px(640.), px(320.)), {
        let terminal = terminal.clone();
        move |window, cx| {
            let state = cx.new(|cx| {
                TerminalState::new(
                    source.borrow_mut().take().expect("one window"),
                    TerminalConfig::default(),
                    window,
                    cx,
                )
            });
            terminal.replace(Some(state.clone()));
            let harness = cx.new(|cx| {
                cx.observe(&state, |_, _, cx| cx.notify()).detach();
                Harness {
                    terminal: state,
                    in_page,
                }
            });
            Root::new(harness, window, cx)
        }
    });
    let terminal = terminal.borrow_mut().take().expect("the terminal");
    let events = Rc::new(RefCell::new(Vec::new()));
    cx.update({
        let events = events.clone();
        let terminal = terminal.clone();
        move |cx| {
            cx.subscribe(&terminal, move |_, event: &TerminalEvent, _| {
                events.borrow_mut().push(event.clone());
            })
            .detach();
        }
    });
    cx.update_window(handle.into(), |_, window, _| window.activate_window())
        .unwrap();
    let setup = Setup {
        handle,
        terminal,
        inputs,
        events,
    };
    settle(&setup, cx);
    setup
}

/// Lets the first frame arrive and the measured grid reach the source.
fn settle(setup: &Setup, cx: &mut TestAppContext) {
    for _ in 0..3 {
        cx.run_until_parked();
        cx.update_window(setup.handle.into(), |_, window, cx| window.render_frame(cx))
            .unwrap();
    }
}

fn inputs(setup: &Setup) -> Vec<TerminalInput> {
    setup.inputs.lock().unwrap().clone()
}

/// The keys pressed, in order; releases are left out.
fn keys(setup: &Setup) -> Vec<String> {
    inputs(setup)
        .into_iter()
        .filter_map(|input| match input {
            TerminalInput::Key(key) if key.action == KeyAction::Press => Some(key.key),
            _ => None,
        })
        .collect()
}

#[gpui_kit::test]
fn the_grid_fills_the_element_and_shows_the_program_output(cx: &mut TestAppContext) {
    let setup = setup(cx);
    let bounds = cx
        .update_window(setup.handle.into(), |_, window, _| {
            window.find("terminal").bounds()
        })
        .unwrap();
    assert_eq!(bounds.size, size(px(640.), px(320.)));
    setup.terminal.read_with(cx, |state, _| {
        let frame = state.frame();
        assert!(frame.text().starts_with("hello world\nsecond line"));
        // The fixture was rebuilt at the measured grid, not the 80x24
        // default, so the columns fit the element.
        assert_ne!(frame.viewport.columns(), 80);
        assert!(frame.viewport.columns() > 20);
        assert!(*state.status() == TerminalStatus::Live);
    });
}

#[gpui_kit::test]
fn the_terminal_is_one_accessibility_node_that_dictation_can_write_to(cx: &mut TestAppContext) {
    let setup = setup(cx);
    cx.update_window(setup.handle.into(), |_, window, _| {
        let node = window.find("terminal");
        assert_eq!(node.role(), Some(gpui_kit::Role::Terminal));
        assert_eq!(node.label(), Some("Terminal"));
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_click_focuses_the_terminal_and_reports_focus(cx: &mut TestAppContext) {
    let setup = setup(cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click("terminal", cx);
    })
    .unwrap();
    settle(&setup, cx);
    assert!(inputs(&setup).contains(&TerminalInput::Focus(true)));
    assert!(setup.events.borrow().contains(&TerminalEvent::Focused));
}

#[gpui_kit::test]
fn init_binds_no_terminal_keys(cx: &mut TestAppContext) {
    let _setup = bare(cx);
    cx.update(|cx| {
        let keymap = cx.key_bindings();
        let keymap = keymap.borrow();
        let terminal: Vec<_> = keymap
            .bindings()
            .filter(|binding| binding.action().name().starts_with("terminal::"))
            .map(|binding| binding.action().name())
            .collect();
        assert!(terminal.is_empty(), "bound by init: {terminal:?}");
    });
}

#[gpui_kit::test]
fn keys_reach_the_program_and_tab_stays_in_the_terminal(cx: &mut TestAppContext) {
    // No terminal bindings: Root's Tab, Shift-Tab and, outside macOS, its
    // Ctrl-C copy key still reach the program.
    let setup = bare(cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click("terminal", cx);
        window.press("a", cx);
        window.press("enter", cx);
        window.press("tab", cx);
        window.press("shift-tab", cx);
        window.press("ctrl-c", cx);
    })
    .unwrap();
    assert_eq!(keys(&setup), ["a", "enter", "tab", "tab", "c"]);
    let focused = cx
        .update_window(setup.handle.into(), |_, window, cx| {
            setup.terminal.focus_handle(cx).is_focused(window)
        })
        .unwrap();
    assert!(focused, "Tab did not move focus out of the terminal");
    let interrupt = inputs(&setup)
        .into_iter()
        .rev()
        .find(TerminalInput::is_interrupt);
    assert!(interrupt.is_some(), "ctrl-c is an interrupt");
}

#[gpui_kit::test]
fn text_composed_in_the_input_method_is_sent_when_committed(cx: &mut TestAppContext) {
    let setup = setup(cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click("terminal", cx);
        setup.terminal.update(cx, |state, cx| {
            state.replace_and_mark_text_in_range(None, "k", None, window, cx);
        });
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(
        setup
            .terminal
            .read_with(cx, |state, _| state.marked_text().map(str::to_owned)),
        Some("k".to_owned())
    );
    cx.update_window(setup.handle.into(), |_, window, cx| {
        setup.terminal.update(cx, |state, cx| {
            state.replace_text_in_range(None, "\u{304b}", window, cx);
        });
    })
    .unwrap();
    assert!(inputs(&setup).contains(&TerminalInput::Text("\u{304b}".into())));
    assert!(
        setup
            .terminal
            .read_with(cx, |state, _| state.marked_text().is_none())
    );
}

#[gpui_kit::test]
fn a_drag_selects_text_that_copy_puts_on_the_clipboard(cx: &mut TestAppContext) {
    let setup = setup(cx);
    drag_copy_and_escape(&setup, true, cx);
}

#[gpui_kit::test]
fn without_bindings_root_copies_the_selection_and_escape_drops_it(cx: &mut TestAppContext) {
    let setup = bare(cx);
    // Root binds a copy key on macOS only: Ctrl-C elsewhere is the program's.
    drag_copy_and_escape(&setup, cfg!(target_os = "macos"), cx);
}

/// Selects "hello" with a drag, copies it when `copy` is set, and drops
/// the selection with Escape, which reaches the program only after.
fn drag_copy_and_escape(setup: &Setup, copy: bool, cx: &mut TestAppContext) {
    let bounds = cx
        .update_window(setup.handle.into(), |_, window, _| {
            window.find("terminal").bounds()
        })
        .unwrap();
    // From the left edge of the first row to well past "hello".
    let row = bounds.origin.y + px(10.);
    let from = point(bounds.origin.x + px(1.), row);
    let cell = setup
        .terminal
        .read_with(cx, |state, _| state.frame().viewport.columns());
    let to = point(
        bounds.origin.x + bounds.size.width * (5.0 / f32::from(cell)) + px(6.),
        row,
    );
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click("terminal", cx);
        window.drag(from, to, cx);
        window.render_frame(cx);
    })
    .unwrap();
    let selected = setup
        .terminal
        .read_with(cx, |state, _| state.selected_text())
        .expect("a selection");
    assert!(selected.starts_with("hello"), "selected {selected:?}");

    if copy {
        cx.update_window(setup.handle.into(), |_, window, cx| {
            window.press(
                if cfg!(target_os = "macos") {
                    "cmd-c"
                } else {
                    "ctrl-shift-c"
                },
                cx,
            );
        })
        .unwrap();
        let copied = cx.read_from_clipboard().and_then(|item| item.text());
        assert_eq!(copied.as_deref(), Some(selected.trim_end()));
    }

    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.press("escape", cx);
    })
    .unwrap();
    assert!(
        !setup
            .terminal
            .read_with(cx, |state, _| state.has_selection())
    );
    assert!(!keys(setup).contains(&"escape".to_owned()), "kept back");
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.press("escape", cx);
    })
    .unwrap();
    assert!(keys(setup).contains(&"escape".to_owned()), "then sent");
}

#[gpui_kit::test]
fn an_exit_stops_input_and_shows_the_status(cx: &mut TestAppContext) {
    let setup = setup(cx);
    cx.update(|cx| {
        setup.terminal.update(cx, |state, cx| {
            let mut snapshot = TerminalSnapshot::for_frame(state.frame().clone());
            snapshot.status = TerminalStatus::Exited(ExitStatus::new(Some(2), None::<String>));
            state.apply(&snapshot, cx);
        });
    });
    settle(&setup, cx);
    assert!(
        setup.events.borrow().iter().any(
            |event| matches!(event, TerminalEvent::Exited(status) if status.code() == Some(2))
        )
    );
    assert!(!setup.terminal.read_with(cx, |state, _| state.is_live()));
    cx.update_window(setup.handle.into(), |_, window, _| {
        let status = ElementId::NamedChild(Arc::new("terminal".into()), "status".into());
        assert!(window.try_find(status).is_some(), "the exit status shows");
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_wheel_over_the_terminal_scrolls_the_terminal_not_the_page(cx: &mut TestAppContext) {
    let setup = setup_with(cx, true, true);
    let top = |cx: &mut TestAppContext| {
        cx.update_window(setup.handle.into(), |_, window, _| {
            window.find("terminal").bounds().origin.y
        })
        .unwrap()
    };
    let before = top(cx);
    for delta in [px(-120.), px(120.)] {
        cx.update_window(setup.handle.into(), |_, window, cx| {
            window.scroll("terminal", ScrollDelta::Pixels(point(px(0.), delta)), cx);
        })
        .unwrap();
        settle(&setup, cx);
        assert_eq!(top(cx), before, "the page stays put");
    }
    let scrolls = inputs(&setup)
        .into_iter()
        .filter(|input| matches!(input, TerminalInput::Scroll(_)))
        .count();
    assert_eq!(scrolls, 2, "both steps scroll the terminal");

    // The same step beside the terminal scrolls the page, so the page
    // could have moved.
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.scroll("above", ScrollDelta::Pixels(point(px(0.), px(-120.))), cx);
    })
    .unwrap();
    settle(&setup, cx);
    assert!(top(cx) < before, "the page scrolls under the pointer");
}

#[gpui_kit::test]
fn the_grid_takes_every_whole_cell_inside_the_padding(cx: &mut TestAppContext) {
    let setup = setup(cx);
    let (bounds, scale, padding) = cx
        .update_window(setup.handle.into(), |_, window, cx| {
            (
                window.find("terminal").bounds(),
                window.scale_factor(),
                gpui_cn::ActiveTheme::theme(cx).metrics.terminal_padding,
            )
        })
        .unwrap();
    let viewport = setup
        .terminal
        .read_with(cx, |state, _| state.frame().viewport);
    let cell_width = viewport.cell_width() as f32 / scale;
    let cell_height = viewport.cell_height() as f32 / scale;
    let room_x = f32::from(bounds.size.width - padding * 2.);
    let room_y = f32::from(bounds.size.height - padding * 2.);
    assert_eq!(
        f32::from(viewport.columns()),
        (room_x / cell_width).floor(),
        "every whole column that fits"
    );
    assert_eq!(
        f32::from(viewport.rows()),
        (room_y / cell_height).floor(),
        "every whole row that fits"
    );
    // What is left over is less than a cell, as in Ghostty.
    assert!(room_x - f32::from(viewport.columns()) * cell_width < cell_width);
    assert!(room_y - f32::from(viewport.rows()) * cell_height < cell_height);
}

/// An application binding for `keys`, built at run time, in the terminal's
/// key context.
fn terminal_binding(
    keys: &str,
    action: impl gpui_kit::Action,
    cx: &gpui_kit::App,
) -> gpui_kit::KeyBinding {
    let context = gpui_kit::KeyBindingContextPredicate::parse(gpui_cn::terminal::KEY_CONTEXT)
        .expect("a valid key context");
    gpui_kit::KeyBinding::load(
        keys,
        Box::new(action),
        Some(Rc::new(context)),
        false,
        None,
        cx,
    )
    .expect("a valid key binding")
}

fn zoom_key(key: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("cmd-{key}")
    } else {
        format!("ctrl-{key}")
    }
}

#[gpui_kit::test]
fn the_font_zoom_keys_resize_the_grid_and_reset_to_the_theme_size(cx: &mut TestAppContext) {
    let setup = setup(cx);
    let theme_size = cx.update(|cx| gpui_cn::ActiveTheme::theme(cx).base.typography.mono_md.size);
    let grid = |cx: &mut TestAppContext| {
        setup.terminal.read_with(cx, |state, cx| {
            (
                state.font_size(cx),
                state.frame().viewport.columns(),
                state.frame().viewport.rows(),
            )
        })
    };
    let (size, columns, rows) = grid(cx);
    assert_eq!(size, theme_size);

    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click("terminal", cx);
        window.press(&zoom_key("="), cx);
        window.press(&zoom_key("="), cx);
    })
    .unwrap();
    settle(&setup, cx);
    let (bigger, fewer_columns, fewer_rows) = grid(cx);
    assert_eq!(bigger, theme_size + px(2.), "two one-point steps");
    assert!(
        fewer_columns < columns && fewer_rows <= rows,
        "the source got the smaller grid"
    );
    assert!(
        setup
            .events
            .borrow()
            .contains(&TerminalEvent::FontSizeChanged)
    );

    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.press(&zoom_key("-"), cx);
        window.press(&zoom_key("-"), cx);
        window.press(&zoom_key("-"), cx);
    })
    .unwrap();
    settle(&setup, cx);
    assert_eq!(grid(cx).0, theme_size - px(1.));

    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.press(&zoom_key("0"), cx);
    })
    .unwrap();
    settle(&setup, cx);
    assert_eq!(
        grid(cx),
        (theme_size, columns, rows),
        "back to the theme's size"
    );

    // Ghostty's limits.
    cx.update(|cx| {
        setup
            .terminal
            .update(cx, |state, cx| state.set_font_size(px(900.), cx));
    });
    assert_eq!(grid(cx).0, px(255.));
}

gpui_kit::actions!(
    app,
    [
        /// An application's own action on a terminal key.
        AppZoom
    ]
);

#[gpui_kit::test]
fn an_app_binding_overrides_or_removes_a_terminal_default(cx: &mut TestAppContext) {
    let setup = setup(cx);
    let theme_size = cx.update(|cx| gpui_cn::ActiveTheme::theme(cx).base.typography.mono_md.size);
    let used = Rc::new(RefCell::new(0));
    cx.update({
        let used = used.clone();
        move |cx| {
            // Bound after gpui_cn::init, in the terminal's context, so it wins.
            cx.bind_keys([
                terminal_binding(&zoom_key("="), AppZoom, cx),
                terminal_binding(&zoom_key("-"), gpui_kit::NoAction, cx),
            ]);
            cx.on_action(move |_: &AppZoom, _| *used.borrow_mut() += 1);
        }
    });
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click("terminal", cx);
        window.press(&zoom_key("="), cx);
        window.press(&zoom_key("-"), cx);
    })
    .unwrap();
    settle(&setup, cx);
    assert_eq!(*used.borrow(), 1, "the app's action ran");
    assert_eq!(
        setup
            .terminal
            .read_with(cx, |state, cx| state.font_size(cx)),
        theme_size,
        "neither default ran"
    );
}

#[gpui_kit::test]
fn without_the_defaults_the_zoom_keys_do_nothing(cx: &mut TestAppContext) {
    let setup = bare(cx);
    let theme_size = cx.update(|cx| gpui_cn::ActiveTheme::theme(cx).base.typography.mono_md.size);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click("terminal", cx);
        window.press(&zoom_key("="), cx);
    })
    .unwrap();
    settle(&setup, cx);
    assert_eq!(
        setup
            .terminal
            .read_with(cx, |state, cx| state.font_size(cx)),
        theme_size
    );
}

#[gpui_kit::test]
fn a_new_foreground_process_is_an_event_and_asks_before_close(cx: &mut TestAppContext) {
    use gpui_cn::terminal::ForegroundProcess;
    let setup = setup(cx);
    let publish = |foreground: ForegroundProcess, cx: &mut TestAppContext| {
        cx.update(|cx| {
            setup.terminal.update(cx, |state, cx| {
                let mut snapshot = TerminalSnapshot::for_frame(state.frame().clone());
                snapshot.foreground = Some(foreground);
                state.apply(&snapshot, cx);
            });
        });
    };
    let changes = |setup: &Setup| {
        setup
            .events
            .borrow()
            .iter()
            .filter(|event| **event == TerminalEvent::ForegroundChanged)
            .count()
    };

    publish(
        ForegroundProcess::new(10, "zsh")
            .with_argv(["-zsh"])
            .with_cwd("/tmp/project")
            .with_shell(true),
        cx,
    );
    assert_eq!(changes(&setup), 1);
    assert!(setup.events.borrow().contains(&TerminalEvent::CwdChanged));
    setup.terminal.read_with(cx, |state, _| {
        assert_eq!(state.foreground().map(|p| p.name()), Some("zsh"));
        assert_eq!(
            state.cwd(),
            Some(std::path::Path::new("/tmp/project")),
            "the process's directory, with nothing reported"
        );
        assert!(!state.needs_confirm_close(), "the shell");
    });

    publish(
        ForegroundProcess::new(11, "vim").with_argv(["vim", "notes.md"]),
        cx,
    );
    assert_eq!(changes(&setup), 2);
    setup.terminal.read_with(cx, |state, _| {
        assert_eq!(
            state.foreground().map(|p| p.argv()[1].as_str()),
            Some("notes.md")
        );
        assert!(state.needs_confirm_close(), "closing would end vim");
    });
}
