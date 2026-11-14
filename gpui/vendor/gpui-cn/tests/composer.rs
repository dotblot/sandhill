//! UI integration tests for `Composer`: real input through a headless
//! window.

use std::{cell::RefCell, path::PathBuf, rc::Rc};

use gpui_cn::{
    Attachment, Composer, ComposerEvent, ComposerState, ComposerStatusTab, ModelEntry,
    ModelPickerState, ModelProvider, ReduceMotion, StatusOption, StatusSelect, StatusSelectState,
    Theme,
};
use gpui_kit::base::Root;
use gpui_kit::{
    AppContext as _, Context, ElementId, Entity, ExternalPaths, FileDropEvent, InputEvent as _,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
    TestAppContext, Window,
    assets::IconName,
    base::{Disableable as _, TestSupportExt as _},
    div, px, size,
    test::TestWindowExt as _,
};

#[derive(Default, Clone)]
struct Options {
    status: bool,
    no_mic: bool,
    custom_leading: bool,
    disabled: bool,
}

struct Harness {
    state: Entity<ComposerState>,
    branch: Entity<StatusSelectState>,
    options: Options,
}

impl Render for Harness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let options = self.options.clone();
        let composer = Composer::new("composer", &self.state)
            .show_mic(!options.no_mic)
            .disabled(options.disabled);
        let composer = if options.status {
            composer.status(
                ComposerStatusTab::new("status")
                    .select(
                        StatusSelect::new("status-branch", &self.branch).icon(IconName::GitBranch),
                    )
                    .context(57.),
            )
        } else {
            composer.no_status()
        };
        let composer = if options.custom_leading {
            composer.leading(div().id("mine").test_support().size(px(20.)))
        } else {
            composer
        };
        div()
            .size_full()
            .p_4()
            .flex()
            .flex_col()
            .items_start()
            .child(div().w(px(560.)).child(composer))
    }
}

struct Setup {
    handle: gpui_kit::WindowHandle<Root>,
    state: Entity<ComposerState>,
    events: Rc<RefCell<Vec<ComposerEvent>>>,
}

fn setup(cx: &mut TestAppContext, options: Options) -> Setup {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
        Theme::update(cx, |theme| theme.reduce_motion = ReduceMotion::On);
    });
    let events = Rc::new(RefCell::new(Vec::new()));
    let mut slot = None;
    let handle = cx.open_window(size(px(800.), px(800.)), |window, cx| {
        let models = cx.new(|cx| {
            ModelPickerState::new(
                [ModelProvider::new("acme", "Acme").models([
                    ModelEntry::new("acme-fast", "Acme Fast"),
                    ModelEntry::new("acme-deep", "Acme Deep").effort(true),
                ])],
                window,
                cx,
            )
        });
        let state = cx.new(|cx| ComposerState::new(window, cx).with_models(models, window, cx));
        let recorded = events.clone();
        cx.subscribe(&state, move |_, _, event: &ComposerEvent, _| {
            recorded.borrow_mut().push(event.clone());
        })
        .detach();
        slot = Some(state.clone());
        let harness = cx.new(|cx| {
            cx.observe(&state, |_, _, cx| cx.notify()).detach();
            let branch = cx.new(|cx| {
                StatusSelectState::new(
                    [
                        StatusOption::new("main", "main"),
                        StatusOption::new("dev", "dev"),
                    ],
                    cx,
                )
            });
            Harness {
                state,
                branch,
                options,
            }
        });
        Root::new(harness, window, cx)
    });
    cx.update_window(handle.into(), |_, window, _| window.activate_window())
        .unwrap();
    cx.run_until_parked();
    cx.update_window(handle.into(), |_, window, cx| window.render_frame(cx))
        .unwrap();
    Setup {
        handle,
        state: slot.unwrap(),
        events,
    }
}

fn card_top_gap(
    tab: gpui_kit::Bounds<gpui_kit::Pixels>,
    card: gpui_kit::Bounds<gpui_kit::Pixels>,
) -> gpui_kit::Pixels {
    card.top() - tab.top()
}

fn part(name: &'static str) -> ElementId {
    ElementId::NamedChild(ElementId::from("composer").into(), name.into())
}

fn nested(parent: ElementId, name: impl Into<SharedString>) -> ElementId {
    ElementId::NamedChild(parent.into(), name.into())
}

fn type_text(setup: &Setup, text: &str, cx: &mut TestAppContext) {
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click(part("text"), cx);
        window.input(text, cx);
        window.render_frame(cx);
    })
    .unwrap();
}

fn text(setup: &Setup, cx: &mut TestAppContext) -> SharedString {
    setup.state.read_with(cx, |state, cx| state.text(cx))
}

fn submits(setup: &Setup) -> Vec<String> {
    setup
        .events
        .borrow()
        .iter()
        .filter_map(|event| match event {
            ComposerEvent::Submit { text, .. } => Some(text.to_string()),
            _ => None,
        })
        .collect()
}

#[gpui_kit::test]
fn enter_submits_the_text_and_clears_it(cx: &mut TestAppContext) {
    let setup = setup(cx, Options::default());
    type_text(&setup, "hello there", cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.press("enter", cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert_eq!(submits(&setup), ["hello there"]);
    assert_eq!(text(&setup, cx), "", "an uncontrolled composer clears");
}

#[gpui_kit::test]
fn shift_enter_starts_a_new_line_and_does_not_submit(cx: &mut TestAppContext) {
    let setup = setup(cx, Options::default());
    type_text(&setup, "one", cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.press("shift-enter", cx);
        window.input("two", cx);
    })
    .unwrap();
    assert_eq!(text(&setup, cx), "one\ntwo");
    assert!(submits(&setup).is_empty());
}

#[gpui_kit::test]
fn an_empty_prompt_does_not_submit_and_the_send_button_is_disabled(cx: &mut TestAppContext) {
    let setup = setup(cx, Options::default());
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click(part("text"), cx);
        window.press("enter", cx);
        window.click(part("send"), cx);
    })
    .unwrap();
    assert!(submits(&setup).is_empty());
    assert!(
        !setup.state.read_with(cx, |state, cx| state.can_submit(cx)),
        "an empty prompt cannot be sent"
    );
    type_text(&setup, "   ", cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.press("enter", cx);
    })
    .unwrap();
    assert!(submits(&setup).is_empty());
    assert!(
        !setup.state.read_with(cx, |state, cx| state.can_submit(cx)),
        "spaces alone cannot be sent"
    );
}

#[gpui_kit::test]
fn the_send_button_submits_once_there_is_text(cx: &mut TestAppContext) {
    let setup = setup(cx, Options::default());
    type_text(&setup, "ship it", cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(part("send"), cx);
    })
    .unwrap();
    assert_eq!(submits(&setup), ["ship it"]);
}

#[gpui_kit::test]
fn a_disabled_composer_ignores_typing_and_sending(cx: &mut TestAppContext) {
    let setup = setup(
        cx,
        Options {
            disabled: true,
            ..Options::default()
        },
    );
    type_text(&setup, "nope", cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.press("enter", cx);
    })
    .unwrap();
    assert_eq!(text(&setup, cx), "");
    assert!(submits(&setup).is_empty());
    assert!(setup.state.read_with(cx, |state, _| state.is_disabled()));
    assert!(!setup.state.read_with(cx, |state, cx| state.can_submit(cx)));
}

#[gpui_kit::test]
fn the_prompt_grows_line_by_line_and_stops_at_ten(cx: &mut TestAppContext) {
    let setup = setup(cx, Options::default());
    let height = |setup: &Setup, cx: &mut TestAppContext| {
        cx.update_window(setup.handle.into(), |_, window, cx| {
            window.render_frame(cx);
            window.find(part("text")).bounds().size.height
        })
        .unwrap()
    };
    let one = height(&setup, cx);
    type_text(&setup, "1", cx);
    for line in 2..=9 {
        cx.update_window(setup.handle.into(), |_, window, cx| {
            window.press("shift-enter", cx);
            window.input(&line.to_string(), cx);
        })
        .unwrap();
    }
    let nine = height(&setup, cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.press("shift-enter", cx);
        window.input("10", cx);
    })
    .unwrap();
    let ten = height(&setup, cx);
    for line in 11..=14 {
        cx.update_window(setup.handle.into(), |_, window, cx| {
            window.press("shift-enter", cx);
            window.input(&line.to_string(), cx);
        })
        .unwrap();
    }
    let fourteen = height(&setup, cx);
    assert!(nine > one, "it grows with the lines");
    assert!(ten > nine, "the tenth line still grows it");
    assert_eq!(fourteen, ten, "the cap is ten lines");
}

#[gpui_kit::test]
fn an_attachment_shows_in_the_strip_and_its_dismiss_button_removes_it(cx: &mut TestAppContext) {
    let setup = setup(cx, Options::default());
    setup.state.update(cx, |state, cx| {
        state.add_attachment(Attachment::new("notes", "notes.md"), cx);
        state.add_attachment(Attachment::new("sheet", "q3.xlsx").progress(40.), cx);
    });
    let tile = |name: &'static str| nested(part("attachments"), name);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        let strip = window.find(part("attachments")).bounds();
        let text = window.find(part("text")).bounds();
        assert!(strip.bottom() <= text.top(), "the strip is above the text");
        assert!(window.try_find(tile("notes")).is_some());
        window.click(nested(tile("notes"), "dismiss"), cx);
        window.render_frame(cx);
        assert!(window.try_find(tile("notes")).is_none());
    })
    .unwrap();
    assert_eq!(
        setup
            .state
            .read_with(cx, |state, _| state.attachments().len()),
        1
    );
    let changes = setup
        .events
        .borrow()
        .iter()
        .filter(|event| matches!(event, ComposerEvent::AttachmentsChanged))
        .count();
    assert_eq!(changes, 3, "two adds and one dismiss");
}

#[gpui_kit::test]
fn an_uploading_attachment_blocks_the_send_until_its_progress_clears(cx: &mut TestAppContext) {
    let setup = setup(cx, Options::default());
    type_text(&setup, "see attached", cx);
    setup.state.update(cx, |state, cx| {
        state.add_attachment(Attachment::new("sheet", "q3.xlsx").progress(40.), cx);
    });
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(part("text"), cx);
        window.press("enter", cx);
    })
    .unwrap();
    assert!(submits(&setup).is_empty(), "still uploading");
    assert!(!setup.state.read_with(cx, |state, cx| state.can_submit(cx)));
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click(part("send"), cx);
    })
    .unwrap();
    assert!(submits(&setup).is_empty(), "the send button is inert too");
    setup.state.update(cx, |state, cx| {
        state.set_attachment_progress("sheet", None, cx);
    });
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(part("send"), cx);
    })
    .unwrap();
    assert_eq!(submits(&setup), ["see attached"]);
    assert!(
        setup
            .state
            .read_with(cx, |state, _| state.attachments().is_empty()),
        "a submit clears the attachments"
    );
    let sent = setup.events.borrow().iter().find_map(|event| match event {
        ComposerEvent::Submit { attachments, .. } => Some(attachments.len()),
        _ => None,
    });
    assert_eq!(sent, Some(1), "the attachment went with the prompt");
}

#[gpui_kit::test]
fn the_permission_menu_changes_the_mode_and_reports_it(cx: &mut TestAppContext) {
    let setup = setup(cx, Options::default());
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click(nested(part("permission"), "trigger"), cx);
        window.render_frame(cx);
        window.render_frame(cx);
        window.click(nested(part("permission"), "plan"), cx);
        window.render_frame(cx);
    })
    .unwrap();
    assert!(
        setup
            .events
            .borrow()
            .iter()
            .any(|event| matches!(event, ComposerEvent::PermissionChanged(id) if id == "plan"))
    );
    let mode = setup.state.read_with(cx, |state, cx| {
        state.permission().read(cx).selected().to_string()
    });
    assert_eq!(mode, "plan", "the state holds the mode the event reported");
}

#[gpui_kit::test]
fn the_model_picker_chooses_a_model_and_an_effort(cx: &mut TestAppContext) {
    let setup = setup(cx, Options::default());
    let open = |setup: &Setup, cx: &mut TestAppContext| {
        cx.update_window(setup.handle.into(), |_, window, cx| {
            window.click(nested(part("models"), "trigger"), cx);
            window.render_frame(cx);
            window.render_frame(cx);
        })
        .unwrap();
        cx.run_until_parked();
    };
    open(&setup, cx);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click(nested(part("models"), "acme-deep"), cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(nested(part("effort"), "trigger"), cx);
        window.render_frame(cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click(nested(part("effort"), "5"), cx);
        window.render_frame(cx);
    })
    .unwrap();
    let (model, effort) = setup.state.read_with(cx, |state, cx| {
        let models = state.models().read(cx);
        (models.selected().cloned(), models.effort())
    });
    assert_eq!(
        model.as_deref(),
        Some("acme-deep"),
        "the state holds the model"
    );
    assert_eq!(effort, Some(5), "and the effort");
    let events = setup.events.borrow();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ComposerEvent::ModelChanged(id) if id == "acme-deep"))
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ComposerEvent::EffortChanged(5)))
    );
}

#[gpui_kit::test]
fn the_add_and_mic_buttons_report_and_the_mic_can_be_hidden(cx: &mut TestAppContext) {
    let setup = setup(cx, Options::default());
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click(part("add"), cx);
        window.click(part("mic"), cx);
    })
    .unwrap();
    let events = setup.events.borrow();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ComposerEvent::AddClicked))
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ComposerEvent::MicClicked))
    );
    drop(events);
    let hidden = self::setup(
        cx,
        Options {
            no_mic: true,
            ..Options::default()
        },
    );
    cx.update_window(hidden.handle.into(), |_, window, _| {
        assert!(window.try_find(part("mic")).is_none());
        assert!(window.try_find(part("send")).is_some());
    })
    .unwrap();
}

#[gpui_kit::test]
fn the_status_tab_sits_behind_the_top_of_the_card(cx: &mut TestAppContext) {
    let with = setup(
        cx,
        Options {
            status: true,
            ..Options::default()
        },
    );
    cx.update_window(with.handle.into(), |_, window, _| {
        let tab = window.find("status").bounds();
        let card = window.find(part("card")).bounds();
        assert_eq!(tab.size.height, px(38.), "the tab has no hidden height");
        assert_eq!(
            card_top_gap(tab, card),
            px(37.),
            "the card tucks over the tab by its hairline"
        );
        assert!(card.top() < tab.bottom(), "the card covers the tab's foot");
        assert_eq!(tab.bottom() - card.top(), px(1.), "by the hairline");
        assert!(card.top() > tab.top());
        assert_eq!(card.size.width, px(560.));
        assert_eq!(tab.left() - card.left(), px(14.), "inset from the card");
    })
    .unwrap();
    let bare = setup(cx, Options::default());
    cx.update_window(bare.handle.into(), |_, window, _| {
        assert!(window.try_find("status").is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
fn a_replaced_control_stands_in_for_the_default(cx: &mut TestAppContext) {
    let setup = setup(
        cx,
        Options {
            custom_leading: true,
            ..Options::default()
        },
    );
    cx.update_window(setup.handle.into(), |_, window, _| {
        assert!(window.try_find(part("add")).is_none());
        assert!(window.try_find("mine").is_some());
    })
    .unwrap();
}

#[gpui_kit::test]
fn dropped_files_are_reported_with_their_paths(cx: &mut TestAppContext) {
    let setup = setup(cx, Options::default());
    cx.update_window(setup.handle.into(), |_, window, cx| {
        let at = window.find(part("card")).bounds().center();
        let paths = ExternalPaths(
            [PathBuf::from("/tmp/a.txt"), PathBuf::from("/tmp/b.png")]
                .into_iter()
                .collect(),
        );
        for event in [
            FileDropEvent::Entered {
                position: at,
                paths,
            },
            FileDropEvent::Pending { position: at },
            FileDropEvent::Submit { position: at },
        ] {
            window.dispatch_event(event.to_platform_input(), cx);
        }
    })
    .unwrap();
    let dropped = setup.events.borrow().iter().find_map(|event| match event {
        ComposerEvent::FilesDropped(paths) => Some(paths.clone()),
        _ => None,
    });
    assert_eq!(
        dropped,
        Some(vec![
            PathBuf::from("/tmp/a.txt"),
            PathBuf::from("/tmp/b.png")
        ])
    );
}

#[gpui_kit::test]
fn attachments_alone_can_be_sent_and_a_queued_or_full_upload_blocks_the_send(
    cx: &mut TestAppContext,
) {
    let setup = setup(cx, Options::default());
    let can_submit = |setup: &Setup, cx: &mut TestAppContext| {
        setup.state.read_with(cx, |state, cx| state.can_submit(cx))
    };
    assert!(!can_submit(&setup, cx), "nothing to send");
    setup.state.update(cx, |state, cx| {
        state.add_attachment(Attachment::new("a", "a.txt"), cx);
    });
    assert!(
        can_submit(&setup, cx),
        "a ready attachment is enough with no text"
    );
    setup.state.update(cx, |state, cx| {
        state.set_attachment_progress("a", Some(0.), cx);
    });
    assert!(!can_submit(&setup, cx), "queued blocks the send");
    setup.state.update(cx, |state, cx| {
        state.set_attachment_progress("a", Some(100.), cx);
    });
    assert!(
        !can_submit(&setup, cx),
        "100 percent still waits for the application to clear the progress"
    );
    setup.state.update(cx, |state, cx| {
        state.set_attachment_progress("a", None, cx);
    });
    assert!(can_submit(&setup, cx), "ready again");
}
