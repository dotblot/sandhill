//! UI integration tests for `FolderPicker`: real input through a headless
//! window, over a fake lister whose answers arrive when the test says.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use gpui_cn::{
    FolderEntry, FolderPage, FolderPicker, FolderPickerEvent, FolderPickerState, FolderSource,
    ListError, Listing, LocalFolders, MoreState, PageToken, PathStyle, ReduceMotion, SourcePath,
    Theme,
};
use gpui_kit::{
    App, AppContext as _, Context, ElementId, Entity, IntoElement, ParentElement as _, Render,
    Styled as _, Task, TestAppContext, Window, WindowHandle, base::Root, div, px, size,
    test::TestWindowExt as _,
};

/// Select all in the path field: Cmd+A on macOS, Ctrl+A elsewhere.
const SELECT_ALL: &str = if cfg!(target_os = "macos") {
    "cmd-a"
} else {
    "ctrl-a"
};

type Reply = Result<FolderPage, ListError>;
type Pending = Vec<(SourcePath, Option<String>, smol::channel::Sender<Reply>)>;

fn sp(text: &str) -> SourcePath {
    PathStyle::posix().path(text)
}

/// A source that answers when told to.
#[derive(Clone, Default)]
struct Fake {
    pending: Rc<RefCell<Pending>>,
    log: Rc<RefCell<Vec<String>>>,
    home: Option<SourcePath>,
}

impl FolderSource for Fake {
    fn home(&self) -> Option<SourcePath> {
        self.home.clone()
    }

    fn path_style(&self) -> PathStyle {
        PathStyle::posix()
    }

    fn list(&self, dir: &SourcePath, page: Option<PageToken>, cx: &mut App) -> Task<Reply> {
        let (tx, rx) = smol::channel::bounded(1);
        let token = page.map(|token| token.as_str().to_string());
        let label = match &token {
            Some(token) => format!("{dir}@{token}"),
            None => dir.to_string(),
        };
        self.log.borrow_mut().push(label);
        self.pending.borrow_mut().push((dir.clone(), token, tx));
        cx.spawn(async move |_| {
            rx.recv()
                .await
                .unwrap_or_else(|_| Err(ListError::Other("cancelled".into())))
        })
    }
}

impl Fake {
    /// Answers the latest request for a page of `dir`. A request whose
    /// task was dropped has nobody listening, which is how a cancel shows.
    fn answer(&self, dir: &str, token: Option<&str>, reply: Reply) {
        let mut pending = self.pending.borrow_mut();
        let at = pending
            .iter()
            .rposition(|(path, page, _)| path == &sp(dir) && page.as_deref() == token)
            .unwrap_or_else(|| panic!("no request for {dir} {token:?}"));
        let (_, _, sender) = pending.remove(at);
        sender.try_send(reply).ok();
    }

    /// Answers the request for the first page of `dir`.
    fn resolve(&self, dir: &str, reply: Reply) {
        self.answer(dir, None, reply);
    }

    /// Answers the request for the page of `dir` that starts at `token`.
    fn resolve_page(&self, dir: &str, token: &str, reply: Reply) {
        self.answer(dir, Some(token), reply);
    }

    fn requested(&self) -> Vec<String> {
        self.log.borrow().clone()
    }
}

fn page(names: &[&str]) -> FolderPage {
    FolderPage::new(names.iter().map(|name| FolderEntry::new(*name)))
}

fn folders(names: &[&str]) -> Reply {
    Ok(page(names))
}

fn more(names: &[&str], token: &str) -> Reply {
    Ok(page(names).with_next(PageToken::new(token)))
}

struct Harness {
    state: Entity<FolderPickerState>,
    open: Rc<Cell<bool>>,
}

impl Render for Harness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let open = self.open.clone();
        div().size_full().child(
            FolderPicker::new("fp", &self.state)
                .open(self.open.get())
                .title("Choose a source folder")
                .on_open_change(move |value, _, _| open.set(value)),
        )
    }
}

struct Setup {
    handle: WindowHandle<Root>,
    state: Entity<FolderPickerState>,
    fake: Fake,
    events: Rc<RefCell<Vec<FolderPickerEvent>>>,
    open: Rc<Cell<bool>>,
}

fn setup(cx: &mut TestAppContext, initial: &str) -> Setup {
    start(cx, Some(initial), None)
}

/// A picker over a fake source with `home`, started at `initial` or, with
/// none, wherever the picker starts by default.
fn start(cx: &mut TestAppContext, initial: Option<&str>, home: Option<&str>) -> Setup {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
        Theme::update(cx, |theme| theme.reduce_motion = ReduceMotion::On);
    });
    let fake = Fake {
        home: home.map(sp),
        ..Fake::default()
    };
    let events = Rc::new(RefCell::new(Vec::new()));
    let open = Rc::new(Cell::new(true));
    let mut state = None;
    let handle = cx.open_window(size(px(800.), px(800.)), |window, cx| {
        let entity = cx.new(|cx| {
            let state = FolderPickerState::new(window, cx).with_source(fake.clone(), window, cx);
            match initial {
                Some(initial) => state.with_initial(initial, window, cx),
                None => state,
            }
        });
        let recorded = events.clone();
        cx.subscribe(&entity, move |_, _, event: &FolderPickerEvent, _| {
            recorded.borrow_mut().push(event.clone());
        })
        .detach();
        state = Some(entity.clone());
        let harness = cx.new(|cx| {
            cx.observe(&entity, |_, _, cx| cx.notify()).detach();
            Harness {
                state: entity,
                open: open.clone(),
            }
        });
        Root::new(harness, window, cx)
    });
    let setup = Setup {
        handle,
        state: state.unwrap(),
        fake,
        events,
        open,
    };
    settle(&setup, cx);
    setup
}

/// Lets tasks and events land, then draws.
fn settle(setup: &Setup, cx: &mut TestAppContext) {
    cx.run_until_parked();
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.activate_window();
        window.render_frame(cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
}

fn type_text(setup: &Setup, text: &str, cx: &mut TestAppContext) {
    cx.update_window(setup.handle.into(), |_, window, cx| window.input(text, cx))
        .unwrap();
    settle(setup, cx);
}

fn press(setup: &Setup, key: &str, cx: &mut TestAppContext) {
    cx.update_window(setup.handle.into(), |_, window, cx| window.press(key, cx))
        .unwrap();
    settle(setup, cx);
}

fn click(setup: &Setup, id: impl Into<ElementId>, cx: &mut TestAppContext) {
    let id = id.into();
    cx.update_window(setup.handle.into(), |_, window, cx| window.click(id, cx))
        .unwrap();
    settle(setup, cx);
}

/// The row of the folder `name`.
fn entry(name: &str) -> ElementId {
    ElementId::NamedChild(part("entry").into(), name.to_string().into())
}

fn part(name: &str) -> ElementId {
    ElementId::NamedChild(
        ElementId::Name("fp".into()).into(),
        gpui_kit::SharedString::from(name.to_string()),
    )
}

fn present(setup: &Setup, id: ElementId, cx: &mut TestAppContext) -> bool {
    cx.update_window(setup.handle.into(), |_, window, _| {
        window.try_find(id).is_some()
    })
    .unwrap()
}

fn text(setup: &Setup, cx: &mut TestAppContext) -> String {
    cx.update(|cx| setup.state.read(cx).text(cx).to_string())
}

fn dir(setup: &Setup, cx: &mut TestAppContext) -> SourcePath {
    cx.update(|cx| setup.state.read(cx).dir().clone())
}

fn highlighted(setup: &Setup, cx: &mut TestAppContext) -> Option<String> {
    cx.update(|cx| {
        setup
            .state
            .read(cx)
            .highlighted()
            .map(|name| name.to_string())
    })
}

fn rows(setup: &Setup, cx: &mut TestAppContext) -> usize {
    cx.update(|cx| setup.state.read(cx).row_count())
}

fn events(setup: &Setup) -> Vec<FolderPickerEvent> {
    setup.events.borrow().clone()
}

const HOME: &[&str] = &["code", ".cache", ".codex", ".config", "docs"];

fn ready(names: &[&str]) -> Reply {
    folders(names)
}

#[gpui_kit::test]
fn a_listing_shows_a_spinner_while_it_loads_and_then_the_rows(cx: &mut TestAppContext) {
    let setup = setup(cx, "/home/me");
    assert_eq!(setup.fake.requested(), ["/home/me"]);
    assert!(present(&setup, part("loading"), cx));
    assert!(!present(&setup, entry("code"), cx));
    cx.update(|cx| {
        assert!(matches!(
            setup.state.read(cx).listing(),
            Some(Listing::Loading)
        ))
    });

    setup.fake.resolve("/home/me", ready(HOME));
    settle(&setup, cx);
    assert!(!present(&setup, part("loading"), cx));
    for name in HOME {
        assert!(present(&setup, entry(name), cx), "{name}");
    }
    assert_eq!(highlighted(&setup, cx).as_deref(), Some("code"));
    cx.update_window(setup.handle.into(), |_, window, _| {
        assert_eq!(window.find(entry("code")).bounds().size.height, px(28.));
    })
    .unwrap();
}

#[gpui_kit::test]
fn typing_filters_fuzzily_and_ranks_the_best_first(cx: &mut TestAppContext) {
    let setup = setup(cx, "/home/me");
    setup.fake.resolve("/home/me", ready(HOME));
    settle(&setup, cx);
    type_text(&setup, "co", cx);
    assert_eq!(text(&setup, cx), "/home/me/co");
    assert_eq!(rows(&setup, cx), 3);
    assert_eq!(highlighted(&setup, cx).as_deref(), Some("code"));
    assert!(!present(&setup, entry("docs"), cx));
    assert!(!present(&setup, entry(".cache"), cx));
    assert_eq!(setup.fake.requested(), ["/home/me"], "no new listing");

    type_text(&setup, "x", cx);
    assert_eq!(rows(&setup, cx), 1);
    assert_eq!(highlighted(&setup, cx).as_deref(), Some(".codex"));

    type_text(&setup, "zz", cx);
    assert_eq!(rows(&setup, cx), 0);
    assert!(present(&setup, part("message"), cx));
}

#[gpui_kit::test]
fn a_separator_goes_inside_the_top_match(cx: &mut TestAppContext) {
    let setup = setup(cx, "/home/me");
    setup.fake.resolve("/home/me", ready(HOME));
    settle(&setup, cx);
    type_text(&setup, "cx", cx);
    type_text(&setup, "/", cx);
    assert_eq!(text(&setup, cx), "/home/me/.codex/");
    assert_eq!(dir(&setup, cx), sp("/home/me/.codex"));
    assert_eq!(setup.fake.requested(), ["/home/me", "/home/me/.codex"]);
    assert_eq!(cx.update(|cx| setup.state.read(cx).query().to_string()), "");
    assert!(present(&setup, part("loading"), cx));
}

#[gpui_kit::test]
fn a_separator_goes_inside_the_exact_folder(cx: &mut TestAppContext) {
    let setup = setup(cx, "/home/me");
    setup
        .fake
        .resolve("/home/me", ready(&["docs", "code", "codex"]));
    settle(&setup, cx);
    type_text(&setup, "code", cx);
    type_text(&setup, "/", cx);
    assert_eq!(text(&setup, cx), "/home/me/code/");
    assert_eq!(dir(&setup, cx), sp("/home/me/code"));
}

#[gpui_kit::test]
fn a_separator_after_no_match_keeps_the_text_and_the_listing_fails(cx: &mut TestAppContext) {
    let setup = setup(cx, "/home/me");
    setup.fake.resolve("/home/me", ready(HOME));
    settle(&setup, cx);
    type_text(&setup, "zzz", cx);
    type_text(&setup, "/", cx);
    assert_eq!(text(&setup, cx), "/home/me/zzz/");
    assert_eq!(setup.fake.requested(), ["/home/me", "/home/me/zzz"]);
    setup
        .fake
        .resolve("/home/me/zzz", Err(ListError::Other("missing".into())));
    settle(&setup, cx);
    cx.update(|cx| {
        assert!(matches!(
            setup.state.read(cx).listing(),
            Some(Listing::Failed(_))
        ))
    });
    assert!(present(&setup, part("message"), cx));
}

#[gpui_kit::test]
fn a_listing_for_a_directory_the_path_left_is_dropped(cx: &mut TestAppContext) {
    let setup = setup(cx, "/home/me");
    click(&setup, part("up"), cx);
    assert_eq!(text(&setup, cx), "/home/");
    assert_eq!(setup.fake.requested(), ["/home/me", "/home"]);

    setup.fake.resolve("/home", ready(&["me", "you"]));
    settle(&setup, cx);
    setup.fake.resolve("/home/me", ready(&["stale"]));
    settle(&setup, cx);
    assert!(present(&setup, entry("me"), cx));
    assert!(!present(&setup, entry("stale"), cx));
    assert_eq!(rows(&setup, cx), 2);

    press(&setup, "enter", cx);
    assert_eq!(text(&setup, cx), "/home/me/");
    assert_eq!(setup.fake.requested(), ["/home/me", "/home", "/home/me"]);
    setup.fake.resolve("/home/me", ready(&["fresh"]));
    settle(&setup, cx);
    click(&setup, part("up"), cx);
    assert_eq!(
        setup.fake.requested().len(),
        3,
        "the cached listing of /home is used again"
    );
    assert_eq!(rows(&setup, cx), 2);
}

#[gpui_kit::test]
fn the_up_button_goes_to_the_parent_and_the_root_stays(cx: &mut TestAppContext) {
    let setup = setup(cx, "/home/me/code");
    assert_eq!(text(&setup, cx), "/home/me/code/");
    click(&setup, part("up"), cx);
    assert_eq!(text(&setup, cx), "/home/me/");
    click(&setup, part("up"), cx);
    click(&setup, part("up"), cx);
    assert_eq!(text(&setup, cx), "/");
    assert_eq!(dir(&setup, cx), sp("/"));
    click(&setup, part("up"), cx);
    assert_eq!(text(&setup, cx), "/");
}

#[gpui_kit::test]
fn enter_and_a_click_go_inside_a_folder(cx: &mut TestAppContext) {
    let setup = setup(cx, "/home/me");
    setup.fake.resolve("/home/me", ready(HOME));
    settle(&setup, cx);
    type_text(&setup, "doc", cx);
    press(&setup, "enter", cx);
    assert_eq!(text(&setup, cx), "/home/me/docs/");

    setup.fake.resolve("/home/me/docs", ready(&["a", "b"]));
    settle(&setup, cx);
    click(&setup, entry("b"), cx);
    assert_eq!(text(&setup, cx), "/home/me/docs/b/");
    assert_eq!(dir(&setup, cx), sp("/home/me/docs/b"));
}

#[gpui_kit::test]
fn the_arrow_keys_move_the_highlight_and_wrap(cx: &mut TestAppContext) {
    let setup = setup(cx, "/home/me");
    setup.fake.resolve("/home/me", ready(&["a", "b", "c"]));
    settle(&setup, cx);
    assert_eq!(highlighted(&setup, cx).as_deref(), Some("a"));
    press(&setup, "down", cx);
    assert_eq!(highlighted(&setup, cx).as_deref(), Some("b"));
    press(&setup, "up", cx);
    press(&setup, "up", cx);
    assert_eq!(highlighted(&setup, cx).as_deref(), Some("c"), "wraps up");
    press(&setup, "down", cx);
    assert_eq!(highlighted(&setup, cx).as_deref(), Some("a"), "wraps down");
    assert!(setup.open.get(), "Enter and arrows never close it");
}

#[gpui_kit::test]
fn use_folder_needs_a_listed_directory_or_an_exact_folder(cx: &mut TestAppContext) {
    let setup = setup(cx, "/home/me");
    click(&setup, part("use"), cx);
    assert!(events(&setup).is_empty());
    assert_eq!(cx.update(|cx| setup.state.read(cx).selected()), None);

    setup.fake.resolve("/home/me", ready(HOME));
    settle(&setup, cx);
    type_text(&setup, "co", cx);
    click(&setup, part("use"), cx);
    assert!(events(&setup).is_empty());

    press(&setup, SELECT_ALL, cx);
    type_text(&setup, "/home/me/docs", cx);
    click(&setup, part("use"), cx);
    assert_eq!(
        events(&setup),
        [FolderPickerEvent::Chosen(sp("/home/me/docs"))]
    );
}

#[gpui_kit::test]
fn the_current_folder_row_and_use_folder_choose_the_listed_directory(cx: &mut TestAppContext) {
    let setup = setup(cx, "/home/me");
    assert!(!present(&setup, part("current"), cx), "not while loading");
    setup.fake.resolve("/home/me", ready(HOME));
    settle(&setup, cx);
    click(&setup, part("current"), cx);
    click(&setup, part("use"), cx);
    assert_eq!(
        events(&setup),
        [
            FolderPickerEvent::Chosen(sp("/home/me")),
            FolderPickerEvent::Chosen(sp("/home/me")),
        ]
    );
    type_text(&setup, "c", cx);
    assert!(!present(&setup, part("current"), cx), "not with a query");
}

#[gpui_kit::test]
fn the_root_is_chosen_with_its_separator(cx: &mut TestAppContext) {
    let setup = setup(cx, "/");
    setup.fake.resolve("/", ready(&["home"]));
    settle(&setup, cx);
    click(&setup, part("use"), cx);
    assert_eq!(events(&setup), [FolderPickerEvent::Chosen(sp("/"))]);
}

#[gpui_kit::test]
fn a_failed_listing_and_an_empty_one_show_a_message(cx: &mut TestAppContext) {
    let setup = setup(cx, "/home/me");
    setup
        .fake
        .resolve("/home/me", Err(ListError::Other("denied".into())));
    settle(&setup, cx);
    cx.update(|cx| {
        assert!(matches!(
            setup.state.read(cx).listing(),
            Some(Listing::Failed(_))
        ))
    });
    assert!(present(&setup, part("message"), cx));
    assert!(!present(&setup, part("loading"), cx));
    click(&setup, part("use"), cx);
    assert!(events(&setup).is_empty(), "a failed folder cannot be used");

    click(&setup, part("up"), cx);
    setup.fake.resolve("/home", ready(&[]));
    settle(&setup, cx);
    cx.update(|cx| {
        assert!(matches!(
            setup.state.read(cx).listing(),
            Some(Listing::Ready(loaded)) if loaded.entries().is_empty()
        ))
    });
    assert!(present(&setup, part("message"), cx));
    click(&setup, part("use"), cx);
    assert_eq!(
        events(&setup),
        [FolderPickerEvent::Chosen(sp("/home"))],
        "an empty folder can be used"
    );
}

#[gpui_kit::test]
fn cancel_and_escape_report_cancelled(cx: &mut TestAppContext) {
    let setup = setup(cx, "/home/me");
    click(&setup, part("cancel"), cx);
    assert_eq!(events(&setup), [FolderPickerEvent::Cancelled]);
    assert!(!setup.open.get());

    let other = self::setup(cx, "/home/me");
    press(&other, "escape", cx);
    assert_eq!(events(&other), [FolderPickerEvent::Cancelled]);
    assert!(!other.open.get());
}

#[gpui_kit::test]
fn a_listing_that_never_answers_leaves_the_dialog_alive(cx: &mut TestAppContext) {
    let setup = setup(cx, "/home/me");
    type_text(&setup, "abc", cx);
    assert_eq!(text(&setup, cx), "/home/me/abc");
    click(&setup, part("up"), cx);
    assert_eq!(text(&setup, cx), "/home/");
    assert!(present(&setup, part("loading"), cx));
    press(&setup, "escape", cx);
    assert_eq!(events(&setup), [FolderPickerEvent::Cancelled]);
}

#[gpui_kit::test]
fn the_field_takes_focus_when_it_opens(cx: &mut TestAppContext) {
    let setup = setup(cx, "/home/me");
    cx.update_window(setup.handle.into(), |_, window, cx| {
        let focus = gpui_kit::Focusable::focus_handle(setup.state.read(cx), cx);
        assert!(focus.is_focused(window));
    })
    .unwrap();
    let cursor = cx.update(|cx| setup.state.read(cx).input().read(cx).cursor());
    assert_eq!(cursor, "/home/me/".len());
}

#[gpui_kit::test]
fn the_local_lister_lists_folders_only_sorted_with_hidden_last(cx: &mut TestAppContext) {
    let root = std::env::temp_dir().join(format!("gpui-cn-folder-picker-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for name in ["beta", "Alpha", ".zed", ".Cache", "gamma"] {
        std::fs::create_dir_all(root.join(name)).unwrap();
    }
    std::fs::write(root.join("file.txt"), "x").unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(root.join("beta"), root.join("link")).unwrap();
        std::os::unix::fs::symlink(root.join("file.txt"), root.join("filelink")).unwrap();
    }
    cx.executor().allow_parking();
    let task = cx
        .update(|cx| LocalFolders.list(&PathStyle::host().path(root.to_string_lossy()), None, cx));
    let listed = cx.foreground_executor().block_test(task).unwrap();
    let listed = listed.entries();
    let names: Vec<_> = listed
        .iter()
        .map(|entry| entry.name().to_string())
        .collect();
    #[cfg(unix)]
    assert_eq!(names, ["Alpha", "beta", "gamma", "link", ".Cache", ".zed"]);
    #[cfg(not(unix))]
    assert_eq!(names, ["Alpha", "beta", "gamma", ".Cache", ".zed"]);
    assert!(
        listed
            .iter()
            .any(|entry| entry.name() == ".zed" && entry.hidden())
    );

    let task = cx.update(|cx| {
        LocalFolders.list(
            &PathStyle::host().path(root.join("missing").to_string_lossy()),
            None,
            cx,
        )
    });
    assert!(cx.foreground_executor().block_test(task).is_err());
    std::fs::remove_dir_all(&root).unwrap();
}

#[gpui_kit::test]
fn a_separator_typed_before_the_filter_finishes_waits_for_it(cx: &mut TestAppContext) {
    let setup = setup(cx, "/home/me");
    setup.fake.resolve("/home/me", ready(HOME));
    settle(&setup, cx);
    for key in ["cx", "/"] {
        cx.update_window(setup.handle.into(), |_, window, cx| window.input(key, cx))
            .unwrap();
    }
    settle(&setup, cx);
    assert_eq!(text(&setup, cx), "/home/me/.codex/");
    assert_eq!(dir(&setup, cx), sp("/home/me/.codex"));
}

const SUBMIT: &str = if cfg!(target_os = "macos") {
    "cmd-enter"
} else {
    "ctrl-enter"
};

#[gpui_kit::test]
fn the_submit_shortcut_chooses_like_use_folder_from_the_path_field(cx: &mut TestAppContext) {
    let setup = setup(cx, "/home/me");
    press(&setup, SUBMIT, cx);
    assert!(events(&setup).is_empty());

    setup.fake.resolve("/home/me", ready(HOME));
    settle(&setup, cx);
    type_text(&setup, "co", cx);
    press(&setup, SUBMIT, cx);
    assert!(events(&setup).is_empty(), "a partial name is no folder");
    assert_eq!(text(&setup, cx), "/home/me/co", "it does not descend");

    press(&setup, SELECT_ALL, cx);
    type_text(&setup, "/home/me/docs", cx);
    press(&setup, SUBMIT, cx);
    assert_eq!(
        events(&setup),
        [FolderPickerEvent::Chosen(sp("/home/me/docs"))]
    );
}

fn more_state(setup: &Setup, cx: &mut TestAppContext) -> Option<MoreState> {
    cx.update(|cx| match setup.state.read(cx).listing() {
        Some(Listing::Ready(loaded)) => Some(loaded.more()),
        _ => None,
    })
}

fn scroll_rows(setup: &Setup, dy: f32, cx: &mut TestAppContext) {
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.scroll(
            part("rows"),
            gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(dy))),
            cx,
        )
    })
    .unwrap();
    settle(setup, cx);
}

#[gpui_kit::test]
fn a_page_with_a_next_token_ends_in_a_load_more_row_that_appends_the_next_page(
    cx: &mut TestAppContext,
) {
    let setup = setup(cx, "/home/me");
    setup.fake.resolve("/home/me", more(&["a", "b"], "2"));
    settle(&setup, cx);
    assert!(present(&setup, part("more"), cx));
    cx.update_window(setup.handle.into(), |_, window, _| {
        assert_eq!(window.find(part("more")).bounds().size.height, px(28.));
    })
    .unwrap();
    assert_eq!(setup.fake.requested(), ["/home/me"]);

    click(&setup, part("more"), cx);
    assert_eq!(setup.fake.requested(), ["/home/me", "/home/me@2"]);
    assert_eq!(more_state(&setup, cx), Some(MoreState::Loading));
    assert!(present(&setup, part("more-spinner"), cx));
    assert_eq!(rows(&setup, cx), 2, "the rows stay while it loads");

    setup
        .fake
        .resolve_page("/home/me", "2", folders(&["b", "c", "d"]));
    settle(&setup, cx);
    assert_eq!(rows(&setup, cx), 4);
    for name in ["a", "b", "c", "d"] {
        assert!(present(&setup, entry(name), cx), "{name}");
    }
    assert!(!present(&setup, part("more"), cx), "no more pages");
    assert_eq!(more_state(&setup, cx), Some(MoreState::Idle));
}

#[gpui_kit::test]
fn a_last_page_has_no_load_more_row(cx: &mut TestAppContext) {
    let setup = setup(cx, "/home/me");
    setup.fake.resolve("/home/me", folders(&["a", "b"]));
    settle(&setup, cx);
    assert!(!present(&setup, part("more"), cx));
}

#[gpui_kit::test]
fn a_page_for_a_directory_the_path_left_is_ignored(cx: &mut TestAppContext) {
    let setup = setup(cx, "/home/me");
    setup.fake.resolve("/home/me", more(&["a"], "2"));
    settle(&setup, cx);
    click(&setup, part("more"), cx);
    click(&setup, part("up"), cx);
    setup.fake.resolve("/home", folders(&["me", "you"]));
    settle(&setup, cx);

    setup
        .fake
        .resolve_page("/home/me", "2", folders(&["stale"]));
    settle(&setup, cx);
    assert_eq!(rows(&setup, cx), 2);
    assert!(!present(&setup, entry("stale"), cx));

    click(&setup, entry("me"), cx);
    assert_eq!(more_state(&setup, cx), Some(MoreState::Idle));
    assert!(present(&setup, entry("a"), cx));
    click(&setup, part("more"), cx);
    assert_eq!(
        setup.fake.requested(),
        ["/home/me", "/home/me@2", "/home", "/home/me@2"]
    );
    setup
        .fake
        .resolve_page("/home/me", "2", folders(&["fresh"]));
    settle(&setup, cx);
    assert_eq!(rows(&setup, cx), 2);
}

#[gpui_kit::test]
fn a_failed_page_keeps_the_rows_and_the_row_retries(cx: &mut TestAppContext) {
    let setup = setup(cx, "/home/me");
    setup.fake.resolve("/home/me", more(&["a", "b"], "2"));
    settle(&setup, cx);
    click(&setup, part("more"), cx);
    setup
        .fake
        .resolve_page("/home/me", "2", Err(ListError::Other("offline".into())));
    settle(&setup, cx);
    assert_eq!(more_state(&setup, cx), Some(MoreState::Failed));
    assert_eq!(rows(&setup, cx), 2, "the loaded folders stay");
    assert!(present(&setup, entry("a"), cx));
    assert!(present(&setup, part("more"), cx));
    assert!(!present(&setup, part("more-spinner"), cx));

    click(&setup, part("more"), cx);
    assert_eq!(more_state(&setup, cx), Some(MoreState::Loading));
    setup.fake.resolve_page("/home/me", "2", folders(&["c"]));
    settle(&setup, cx);
    assert_eq!(rows(&setup, cx), 3);
    assert!(!present(&setup, part("more"), cx));
}

#[gpui_kit::test]
fn enter_on_the_load_more_row_loads_the_next_page(cx: &mut TestAppContext) {
    let setup = setup(cx, "/home/me");
    setup.fake.resolve("/home/me", more(&["a"], "2"));
    settle(&setup, cx);
    press(&setup, "down", cx);
    assert_eq!(highlighted(&setup, cx), None, "the row is no folder");
    press(&setup, "enter", cx);
    assert_eq!(setup.fake.requested(), ["/home/me", "/home/me@2"]);
}

#[gpui_kit::test]
fn the_filter_reaches_a_folder_that_only_a_later_page_has(cx: &mut TestAppContext) {
    let setup = setup(cx, "/home/me");
    setup
        .fake
        .resolve("/home/me", more(&["alpha", "beta"], "2"));
    settle(&setup, cx);
    type_text(&setup, "zu", cx);
    assert_eq!(rows(&setup, cx), 0);
    assert!(
        present(&setup, part("more"), cx),
        "the row stays while a query has no match and more follow"
    );
    assert!(!present(&setup, part("message"), cx));

    click(&setup, part("more"), cx);
    setup.fake.resolve_page("/home/me", "2", folders(&["zulu"]));
    settle(&setup, cx);
    assert_eq!(rows(&setup, cx), 1);
    assert_eq!(highlighted(&setup, cx).as_deref(), Some("zulu"));
    assert!(present(&setup, entry("zulu"), cx));
}

#[gpui_kit::test]
fn reaching_the_end_loads_one_page_and_not_again_while_it_loads(cx: &mut TestAppContext) {
    let setup = setup(cx, "/home/me");
    let names: Vec<String> = (0..30).map(|n| format!("folder-{n:02}")).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    setup.fake.resolve("/home/me", more(&names, "2"));
    settle(&setup, cx);
    assert_eq!(setup.fake.requested(), ["/home/me"], "no load at the top");

    for _ in 0..3 {
        scroll_rows(&setup, -2000., cx);
    }
    assert_eq!(
        setup.fake.requested(),
        ["/home/me", "/home/me@2"],
        "exactly one request, and none while it loads"
    );
    assert_eq!(more_state(&setup, cx), Some(MoreState::Loading));

    setup
        .fake
        .resolve_page("/home/me", "2", more(&["next-page"], "3"));
    settle(&setup, cx);
    assert_eq!(rows(&setup, cx), 31);
    assert_eq!(
        setup.fake.requested().len(),
        2,
        "the row asks nothing by itself"
    );
}

#[gpui_kit::test]
fn a_failed_page_is_not_retried_by_scrolling(cx: &mut TestAppContext) {
    let setup = setup(cx, "/home/me");
    let names: Vec<String> = (0..30).map(|n| format!("folder-{n:02}")).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    setup.fake.resolve("/home/me", more(&names, "2"));
    settle(&setup, cx);
    for _ in 0..3 {
        scroll_rows(&setup, -2000., cx);
    }
    setup
        .fake
        .resolve_page("/home/me", "2", Err(ListError::Other("offline".into())));
    settle(&setup, cx);
    scroll_rows(&setup, -2000., cx);
    scroll_rows(&setup, 50., cx);
    scroll_rows(&setup, -2000., cx);
    assert_eq!(setup.fake.requested().len(), 2);
    assert_eq!(more_state(&setup, cx), Some(MoreState::Failed));
}

#[gpui_kit::test]
fn a_page_that_never_answers_leaves_the_dialog_alive(cx: &mut TestAppContext) {
    let setup = setup(cx, "/home/me");
    setup.fake.resolve("/home/me", more(&["a", "b"], "2"));
    settle(&setup, cx);
    click(&setup, part("more"), cx);
    type_text(&setup, "a", cx);
    assert_eq!(rows(&setup, cx), 1);
    press(&setup, "escape", cx);
    assert_eq!(events(&setup), [FolderPickerEvent::Cancelled]);
}

/// Where the footer's Cancel button is: the dialog is centered, so its
/// height moves this when it changes.
fn footer_top(setup: &Setup, cx: &mut TestAppContext) -> gpui_kit::Pixels {
    cx.update_window(setup.handle.into(), |_, window, _| {
        window.find(part("cancel")).bounds().top()
    })
    .unwrap()
}

fn list_height(setup: &Setup, cx: &mut TestAppContext) -> gpui_kit::Pixels {
    cx.update_window(setup.handle.into(), |_, window, _| {
        window.find(part("list")).bounds().size.height
    })
    .unwrap()
}

#[gpui_kit::test]
fn the_dialog_and_the_list_keep_their_height_in_every_state(cx: &mut TestAppContext) {
    let setup = setup(cx, "/home/me");
    let list = list_height(&setup, cx);
    let top = footer_top(&setup, cx);
    let same = |setup: &Setup, cx: &mut TestAppContext, state: &str| {
        assert_eq!(list_height(setup, cx), list, "list height: {state}");
        assert_eq!(footer_top(setup, cx), top, "dialog height: {state}");
    };
    same(&setup, cx, "loading");

    let fifty: Vec<String> = (0..50).map(|n| format!("folder-{n:02}")).collect();
    let fifty: Vec<&str> = fifty.iter().map(String::as_str).collect();
    setup.fake.resolve("/home/me", ready(&fifty));
    settle(&setup, cx);
    same(&setup, cx, "fifty rows");

    type_text(&setup, "folder-07", cx);
    same(&setup, cx, "one match");
    type_text(&setup, "zzzz", cx);
    same(&setup, cx, "no match");
    for _ in 0.."zzzz".len() {
        press(&setup, "backspace", cx);
    }
    same(&setup, cx, "filter narrowed back to one match");

    click(&setup, part("up"), cx);
    same(&setup, cx, "parent loading");
    setup.fake.resolve("/home", ready(&["me", "new"]));
    settle(&setup, cx);
    same(&setup, cx, "one row");

    click(&setup, entry("new"), cx);
    same(&setup, cx, "descend loading");
    setup
        .fake
        .resolve("/home/new", Err(ListError::Other("denied".into())));
    settle(&setup, cx);
    same(&setup, cx, "failed");

    click(&setup, part("up"), cx);
    click(&setup, part("up"), cx);
    setup.fake.resolve("/", ready(&[]));
    settle(&setup, cx);
    same(&setup, cx, "empty");
}

#[gpui_kit::test]
fn a_page_that_loads_more_keeps_the_height(cx: &mut TestAppContext) {
    let setup = setup(cx, "/home/me");
    setup
        .fake
        .resolve("/home/me", more(&["a", "b", "c"], "next"));
    settle(&setup, cx);
    let (list, top) = (list_height(&setup, cx), footer_top(&setup, cx));
    setup.state.update(cx, |state, cx| {
        state.load_more(cx);
    });
    settle(&setup, cx);
    assert_eq!(list_height(&setup, cx), list, "while the next page loads");
    assert_eq!(footer_top(&setup, cx), top);
    setup.fake.resolve_page("/home/me", "next", folders(&["d"]));
    settle(&setup, cx);
    assert_eq!(footer_top(&setup, cx), top, "after the next page");
}

#[gpui_kit::test]
fn a_picker_with_no_start_opens_at_the_sources_home(cx: &mut TestAppContext) {
    let setup = start(cx, None, Some("/Users/me"));
    assert_eq!(setup.fake.requested(), ["/Users/me"]);
    assert_eq!(text(&setup, cx), "~/", "the field shows the tilde form");
    assert_eq!(dir(&setup, cx), sp("/Users/me"));
}

#[gpui_kit::test]
fn a_source_without_a_home_starts_at_the_root(cx: &mut TestAppContext) {
    let setup = start(cx, None, None);
    assert_eq!(setup.fake.requested(), ["/"]);
    assert_eq!(text(&setup, cx), "/");
}

#[gpui_kit::test]
fn an_explicit_start_wins_over_the_home(cx: &mut TestAppContext) {
    let setup = start(cx, Some("/srv/data"), Some("/Users/me"));
    assert_eq!(dir(&setup, cx), sp("/srv/data"));
    assert_eq!(text(&setup, cx), "/srv/data/");
}

#[gpui_kit::test]
fn a_typed_tilde_lists_the_home_and_a_tilde_path_its_child(cx: &mut TestAppContext) {
    let setup = start(cx, Some("/"), Some("/Users/me"));
    setup.fake.resolve("/", ready(&["Users"]));
    settle(&setup, cx);
    press(&setup, SELECT_ALL, cx);
    type_text(&setup, "~", cx);
    assert_eq!(text(&setup, cx), "~", "the user's own text stays");
    assert_eq!(dir(&setup, cx), sp("/Users/me"));
    setup.fake.resolve("/Users/me", ready(&["code", "docs"]));
    settle(&setup, cx);
    assert!(present(&setup, entry("code"), cx));

    type_text(&setup, "/", cx);
    assert_eq!(text(&setup, cx), "~/");
    assert_eq!(dir(&setup, cx), sp("/Users/me"));
    type_text(&setup, "code/", cx);
    assert_eq!(text(&setup, cx), "~/code/");
    assert_eq!(dir(&setup, cx), sp("/Users/me/code"));
    assert_eq!(
        setup.fake.requested().last().map(String::as_str),
        Some("/Users/me/code")
    );
    setup.fake.resolve("/Users/me/code", ready(&["gpui-cn"]));
    settle(&setup, cx);
    assert!(present(&setup, entry("gpui-cn"), cx));
    click(&setup, part("use"), cx);
    assert_eq!(
        events(&setup),
        [FolderPickerEvent::Chosen(sp("/Users/me/code"))],
        "the chosen path is the real one"
    );
}

#[gpui_kit::test]
fn a_source_without_a_home_does_not_expand_the_tilde(cx: &mut TestAppContext) {
    let setup = start(cx, Some("/"), None);
    setup.fake.resolve("/", ready(&["Users"]));
    settle(&setup, cx);
    press(&setup, SELECT_ALL, cx);
    type_text(&setup, "~/", cx);
    assert_eq!(dir(&setup, cx), sp("~"), "a plain name");
    setup
        .fake
        .resolve("~", Err(ListError::Other("no such folder".into())));
    settle(&setup, cx);
    cx.update(|cx| {
        assert!(matches!(
            setup.state.read(cx).listing(),
            Some(Listing::Failed(_))
        ))
    });
    assert!(present(&setup, part("message"), cx), "the not-found state");
}

#[gpui_kit::test]
fn clicking_inside_home_shows_the_tilde_form_and_up_goes_back_out(cx: &mut TestAppContext) {
    let setup = start(cx, Some("/Users/me"), Some("/Users/me"));
    setup.fake.resolve("/Users/me", ready(&["code"]));
    settle(&setup, cx);
    assert_eq!(
        text(&setup, cx),
        "/Users/me/",
        "the typed start stays as given"
    );
    click(&setup, entry("code"), cx);
    assert_eq!(text(&setup, cx), "~/code/", "a click shows the tilde form");
    assert_eq!(dir(&setup, cx), sp("/Users/me/code"));
    setup.fake.resolve("/Users/me/code", ready(&["gpui-cn"]));
    settle(&setup, cx);
    click(&setup, part("up"), cx);
    assert_eq!(text(&setup, cx), "~/");
    assert_eq!(dir(&setup, cx), sp("/Users/me"));
    click(&setup, part("up"), cx);
    assert_eq!(
        text(&setup, cx),
        "/Users/",
        "up from home goes to its parent"
    );
    assert_eq!(dir(&setup, cx), sp("/Users"));
}
