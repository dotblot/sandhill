//! UI integration tests for `FilePicker`: real input through a headless
//! window, over an in-memory source and a fake lister whose answers arrive
//! when the test says.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use gpui_cn::{
    FileEntry, FileFilter, FilePage, FilePicker, FilePickerEvent, FilePickerState, FileSource,
    ListError, LocalFiles, MemoryFiles, PageToken, PathStyle, ReduceMotion, SourcePath, Theme,
};
use gpui_kit::{
    App, AppContext as _, Context, ElementId, Entity, IntoElement, Modifiers, ParentElement as _,
    Render, SharedString, Styled as _, Task, TestAppContext, Window, WindowHandle, base::Root, div,
    px, size, test::TestWindowExt as _,
};

const SELECT_ALL: &str = if cfg!(target_os = "macos") {
    "cmd-a"
} else {
    "ctrl-a"
};
const SUBMIT: &str = if cfg!(target_os = "macos") {
    "cmd-enter"
} else {
    "ctrl-enter"
};

type Reply = Result<FilePage, ListError>;
type Pending = Vec<(SourcePath, smol::channel::Sender<Reply>)>;

fn sp(text: &str) -> SourcePath {
    PathStyle::posix().path(text)
}

/// A source that answers when told to.
#[derive(Clone, Default)]
struct Fake {
    pending: Rc<RefCell<Pending>>,
}

impl FileSource for Fake {
    fn path_style(&self) -> PathStyle {
        PathStyle::posix()
    }

    fn list(&self, dir: &SourcePath, _: Option<PageToken>, cx: &mut App) -> Task<Reply> {
        let (tx, rx) = smol::channel::bounded(1);
        self.pending.borrow_mut().push((dir.clone(), tx));
        cx.spawn(async move |_| {
            rx.recv()
                .await
                .unwrap_or_else(|_| Err(ListError::Other("cancelled".into())))
        })
    }
}

impl Fake {
    fn resolve(&self, dir: &str, reply: Reply) {
        let mut pending = self.pending.borrow_mut();
        let at = pending
            .iter()
            .rposition(|(path, _)| path == &sp(dir))
            .unwrap_or_else(|| panic!("no request for {dir}"));
        pending.remove(at).1.try_send(reply).ok();
    }
}

fn files(names: &[&str]) -> Reply {
    Ok(FilePage::new(
        names.iter().map(|name| FileEntry::file(*name)),
    ))
}

struct Harness {
    state: Entity<FilePickerState>,
    open: Rc<Cell<bool>>,
}

impl Render for Harness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let open = self.open.clone();
        div().size_full().child(
            FilePicker::new("fp", &self.state)
                .open(self.open.get())
                .on_open_change(move |value, _, _| open.set(value)),
        )
    }
}

struct Setup {
    handle: WindowHandle<Root>,
    state: Entity<FilePickerState>,
    events: Rc<RefCell<Vec<FilePickerEvent>>>,
    open: Rc<Cell<bool>>,
}

type Configure =
    Box<dyn FnOnce(FilePickerState, &mut Window, &mut Context<FilePickerState>) -> FilePickerState>;

fn start(
    cx: &mut TestAppContext,
    source: impl FileSource,
    initial: &str,
    configure: Configure,
) -> Setup {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
        Theme::update(cx, |theme| theme.reduce_motion = ReduceMotion::On);
    });
    let events = Rc::new(RefCell::new(Vec::new()));
    let open = Rc::new(Cell::new(true));
    let mut state = None;
    let handle = cx.open_window(size(px(800.), px(800.)), |window, cx| {
        let entity = cx.new(|cx| {
            let picker = FilePickerState::new(window, cx)
                .with_source(source, window, cx)
                .with_initial(initial, window, cx);
            configure(picker, window, cx)
        });
        let recorded = events.clone();
        cx.subscribe(&entity, move |_, _, event: &FilePickerEvent, _| {
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
        events,
        open,
    };
    settle(&setup, cx);
    setup
}

fn keep() -> Configure {
    Box::new(|state, _, _| state)
}

fn multiple() -> Configure {
    Box::new(|state, _, _| state.with_multiple(true))
}

fn project() -> MemoryFiles {
    MemoryFiles::new()
        .with_style(PathStyle::posix())
        .with_home("/home/me")
        .with_dir(
            "/home/me",
            [
                FileEntry::file("readme.md").with_size(1200),
                FileEntry::file("main.rs").with_size(2048),
                FileEntry::file("Cargo.toml").with_size(300),
                FileEntry::file("notes.txt"),
                FileEntry::file(".env"),
                FileEntry::folder("src"),
                FileEntry::folder(".git"),
                FileEntry::file("lib.rs"),
            ],
        )
        .with_dir("/home/me/src", [FileEntry::file("inner.rs")])
}

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

fn press(setup: &Setup, key: &str, cx: &mut TestAppContext) {
    cx.update_window(setup.handle.into(), |_, window, cx| window.press(key, cx))
        .unwrap();
    settle(setup, cx);
}

fn type_text(setup: &Setup, text: &str, cx: &mut TestAppContext) {
    cx.update_window(setup.handle.into(), |_, window, cx| window.input(text, cx))
        .unwrap();
    settle(setup, cx);
}

fn click(setup: &Setup, id: impl Into<ElementId>, cx: &mut TestAppContext) {
    let id = id.into();
    cx.update_window(setup.handle.into(), |_, window, cx| window.click(id, cx))
        .unwrap();
    settle(setup, cx);
}

fn double_click(setup: &Setup, id: impl Into<ElementId>, cx: &mut TestAppContext) {
    let id = id.into();
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.double_click(id, cx)
    })
    .unwrap();
    settle(setup, cx);
}

/// A click with `modifiers` held, which the window helper cannot send.
fn click_with(setup: &Setup, id: ElementId, modifiers: Modifiers, cx: &mut TestAppContext) {
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.click_with_modifiers(id, modifiers, cx)
    })
    .unwrap();
    settle(setup, cx);
}

fn part(name: &str) -> ElementId {
    ElementId::NamedChild(
        ElementId::Name("fp".into()).into(),
        SharedString::from(name.to_string()),
    )
}

fn entry(name: &str) -> ElementId {
    ElementId::NamedChild(part("entry").into(), SharedString::from(name.to_string()))
}

fn present(setup: &Setup, id: ElementId, cx: &mut TestAppContext) -> bool {
    cx.update_window(setup.handle.into(), |_, window, _| {
        window.try_find(id).is_some()
    })
    .unwrap()
}

fn names(setup: &Setup, cx: &mut TestAppContext) -> Vec<String> {
    setup.state.read_with(cx, |state, _| {
        state.names().iter().map(|name| name.to_string()).collect()
    })
}

fn selection(setup: &Setup, cx: &mut TestAppContext) -> Vec<String> {
    setup.state.read_with(cx, |state, _| {
        state
            .selection()
            .iter()
            .map(|path| path.file_name().unwrap().to_string())
            .collect()
    })
}

fn confirmed(setup: &Setup) -> Vec<Vec<SourcePath>> {
    setup
        .events
        .borrow()
        .iter()
        .filter_map(|event| match event {
            FilePickerEvent::Confirmed(paths) => Some(paths.clone()),
            _ => None,
        })
        .collect()
}

#[gpui_kit::test]
fn folders_come_first_and_hidden_entries_wait_for_the_switch(cx: &mut TestAppContext) {
    let setup = start(cx, project(), "/home/me", keep());
    assert_eq!(
        names(&setup, cx),
        [
            "src",
            "Cargo.toml",
            "lib.rs",
            "main.rs",
            "notes.txt",
            "readme.md"
        ]
    );
    click(&setup, part("hidden"), cx);
    assert!(setup.state.read_with(cx, |state, _| state.show_hidden()));
    assert_eq!(
        names(&setup, cx),
        [
            "src",
            ".git",
            "Cargo.toml",
            "lib.rs",
            "main.rs",
            "notes.txt",
            "readme.md",
            ".env"
        ]
    );
    click(&setup, part("hidden"), cx);
    assert!(!names(&setup, cx).contains(&".env".to_string()));
}

#[gpui_kit::test]
fn a_click_selects_a_file_and_enter_chooses_it(cx: &mut TestAppContext) {
    let setup = start(cx, project(), "/home/me", keep());
    click(&setup, entry("main.rs"), cx);
    assert_eq!(selection(&setup, cx), ["main.rs"]);
    assert!(
        setup
            .events
            .borrow()
            .contains(&FilePickerEvent::SelectionChanged)
    );
    assert!(confirmed(&setup).is_empty(), "a click only selects");
    press(&setup, "enter", cx);
    assert_eq!(confirmed(&setup), [[sp("/home/me/main.rs")]]);
}

#[gpui_kit::test]
fn enter_opens_the_file_the_keyboard_is_on(cx: &mut TestAppContext) {
    let setup = start(cx, project(), "/home/me", keep());
    click(&setup, entry("main.rs"), cx);
    press(&setup, "down", cx);
    assert_eq!(
        setup.state.read_with(cx, |state, _| state.highlighted()),
        Some("notes.txt".into())
    );
    press(&setup, "enter", cx);
    assert_eq!(confirmed(&setup), [[sp("/home/me/notes.txt")]]);
}

#[gpui_kit::test]
fn the_open_button_waits_for_a_selection(cx: &mut TestAppContext) {
    let setup = start(cx, project(), "/home/me", keep());
    click(&setup, part("open"), cx);
    setup.state.update(cx, |state, cx| state.confirm(cx));
    press(&setup, SUBMIT, cx);
    assert!(confirmed(&setup).is_empty(), "nothing is selected");
    click(&setup, entry("lib.rs"), cx);
    click(&setup, part("open"), cx);
    assert_eq!(confirmed(&setup), [[sp("/home/me/lib.rs")]]);
}

#[gpui_kit::test]
fn a_double_click_chooses_a_file(cx: &mut TestAppContext) {
    let setup = start(cx, project(), "/home/me", keep());
    double_click(&setup, entry("readme.md"), cx);
    assert_eq!(confirmed(&setup), [[sp("/home/me/readme.md")]]);
}

#[gpui_kit::test]
fn folders_open_on_a_double_click_or_enter_and_a_click_does_not(cx: &mut TestAppContext) {
    let setup = start(cx, project(), "/home/me", keep());
    click(&setup, entry("src"), cx);
    assert_eq!(
        setup.state.read_with(cx, |state, _| state.dir().clone()),
        sp("/home/me")
    );
    assert!(
        selection(&setup, cx).is_empty(),
        "a folder is never selected"
    );
    double_click(&setup, entry("src"), cx);
    assert_eq!(
        setup.state.read_with(cx, |state, _| state.dir().clone()),
        sp("/home/me/src")
    );
    assert_eq!(names(&setup, cx), ["inner.rs"]);
    click(&setup, part("up"), cx);
    assert_eq!(names(&setup, cx).first().map(String::as_str), Some("src"));
    press(&setup, "enter", cx);
    assert_eq!(
        setup.state.read_with(cx, |state, _| state.dir().clone()),
        sp("/home/me/src")
    );
}

#[gpui_kit::test]
fn a_tap_opens_a_folder_on_touch(cx: &mut TestAppContext) {
    let setup = start(cx, project(), "/home/me", keep());
    cx.update(|cx| Theme::update(cx, |theme| theme.touch = true));
    settle(&setup, cx);
    click(&setup, entry("src"), cx);
    assert_eq!(names(&setup, cx), ["inner.rs"]);
}

#[gpui_kit::test]
fn the_selection_belongs_to_its_directory(cx: &mut TestAppContext) {
    let setup = start(cx, project(), "/home/me", keep());
    click(&setup, entry("lib.rs"), cx);
    double_click(&setup, entry("src"), cx);
    assert!(selection(&setup, cx).is_empty());
    click(&setup, part("up"), cx);
    assert!(selection(&setup, cx).is_empty(), "it does not come back");
}

#[gpui_kit::test]
fn modifiers_toggle_and_extend_a_selection_of_several(cx: &mut TestAppContext) {
    let setup = start(cx, project(), "/home/me", multiple());
    click(&setup, entry("lib.rs"), cx);
    click_with(
        &setup,
        entry("notes.txt"),
        cx.update(|cx| Modifiers::secondary_key(cx)),
        cx,
    );
    assert_eq!(selection(&setup, cx), ["lib.rs", "notes.txt"]);
    click_with(
        &setup,
        entry("lib.rs"),
        cx.update(|cx| Modifiers::secondary_key(cx)),
        cx,
    );
    assert_eq!(selection(&setup, cx), ["notes.txt"], "the modifier toggles");
    click(&setup, entry("Cargo.toml"), cx);
    assert_eq!(
        selection(&setup, cx),
        ["Cargo.toml"],
        "a plain click replaces"
    );
    let shift = Modifiers {
        shift: true,
        ..Modifiers::default()
    };
    click_with(&setup, entry("notes.txt"), shift, cx);
    assert_eq!(
        selection(&setup, cx),
        ["Cargo.toml", "lib.rs", "main.rs", "notes.txt"],
        "shift takes the range, and a folder is no part of it"
    );
    click_with(&setup, entry("lib.rs"), shift, cx);
    assert_eq!(
        selection(&setup, cx),
        ["Cargo.toml", "lib.rs"],
        "from the same anchor"
    );
    press(&setup, SUBMIT, cx);
    assert_eq!(
        confirmed(&setup),
        [[sp("/home/me/Cargo.toml"), sp("/home/me/lib.rs")]]
    );
}

#[gpui_kit::test]
fn a_picker_of_one_file_ignores_modifiers(cx: &mut TestAppContext) {
    let setup = start(cx, project(), "/home/me", keep());
    click(&setup, entry("lib.rs"), cx);
    click_with(
        &setup,
        entry("notes.txt"),
        cx.update(|cx| Modifiers::secondary_key(cx)),
        cx,
    );
    assert_eq!(selection(&setup, cx), ["notes.txt"]);
    press(&setup, SELECT_ALL, cx);
    assert_eq!(selection(&setup, cx), ["notes.txt"]);
}

#[gpui_kit::test]
fn select_all_takes_every_file_until_there_is_a_query(cx: &mut TestAppContext) {
    let setup = start(cx, project(), "/home/me", multiple());
    press(&setup, SELECT_ALL, cx);
    assert_eq!(
        selection(&setup, cx),
        ["Cargo.toml", "lib.rs", "main.rs", "notes.txt", "readme.md"]
    );
    setup
        .state
        .update(cx, |state, cx| state.clear_selection(cx));
    type_text(&setup, "rs", cx);
    press(&setup, SELECT_ALL, cx);
    assert!(
        selection(&setup, cx).is_empty(),
        "the key selects the query text"
    );
}

#[gpui_kit::test]
fn the_open_button_names_the_mode(cx: &mut TestAppContext) {
    let one = start(cx, project(), "/home/me", keep());
    let many = start(cx, project(), "/home/me", multiple());
    let label = |setup: &Setup, cx: &mut TestAppContext| {
        cx.update_window(setup.handle.into(), |_, window, _| {
            window.find(part("open")).label().map(|l| l.to_string())
        })
        .unwrap()
    };
    assert_eq!(label(&one, cx).as_deref(), Some("Open file"));
    assert_eq!(label(&many, cx).as_deref(), Some("Open files"));
}

fn filters() -> Configure {
    Box::new(|state, _, cx| {
        state.with_filters(
            vec![
                FileFilter::new("All files", Vec::<String>::new()),
                FileFilter::new("Rust files", ["rs"]),
            ],
            cx,
        )
    })
}

#[gpui_kit::test]
fn a_filter_hides_other_files_and_keeps_folders(cx: &mut TestAppContext) {
    let setup = start(cx, project(), "/home/me", filters());
    assert_eq!(
        names(&setup, cx).len(),
        6,
        "the first filter shows every file"
    );
    setup
        .state
        .update(cx, |state, cx| state.set_active_filter(1, cx));
    settle(&setup, cx);
    assert_eq!(names(&setup, cx), ["src", "lib.rs", "main.rs"]);
    setup
        .state
        .update(cx, |state, cx| state.set_active_filter(0, cx));
    settle(&setup, cx);
    assert_eq!(names(&setup, cx).len(), 6);
}

#[gpui_kit::test]
fn one_filter_is_named_and_several_offer_a_menu(cx: &mut TestAppContext) {
    let one = start(
        cx,
        project(),
        "/home/me",
        Box::new(|state, _, cx| {
            state.with_filters(vec![FileFilter::new("Rust files", ["rs"])], cx)
        }),
    );
    assert!(present(&one, part("filter"), cx));
    assert!(!present(&one, part("filter-trigger"), cx));
    let many = start(cx, project(), "/home/me", filters());
    assert!(present(&many, part("filter-trigger"), cx));
}

#[gpui_kit::test]
fn the_menu_switches_the_filter(cx: &mut TestAppContext) {
    let setup = start(cx, project(), "/home/me", filters());
    click(&setup, part("filter-trigger"), cx);
    settle(&setup, cx);
    click(
        &setup,
        ElementId::NamedChild(part("filter-menu").into(), "1".into()),
        cx,
    );
    assert_eq!(
        setup
            .state
            .read_with(cx, |state, _| state.active_filter_index()),
        1
    );
    assert_eq!(names(&setup, cx), ["src", "lib.rs", "main.rs"]);
}

#[gpui_kit::test]
fn a_selected_file_the_filter_hides_is_not_chosen(cx: &mut TestAppContext) {
    let setup = start(cx, project(), "/home/me", multiple_filters());
    click(&setup, entry("lib.rs"), cx);
    click_with(
        &setup,
        entry("notes.txt"),
        cx.update(|cx| Modifiers::secondary_key(cx)),
        cx,
    );
    setup
        .state
        .update(cx, |state, cx| state.set_active_filter(1, cx));
    settle(&setup, cx);
    assert_eq!(selection(&setup, cx), ["lib.rs"]);
}

fn multiple_filters() -> Configure {
    Box::new(|state, _, cx| {
        state.with_multiple(true).with_filters(
            vec![
                FileFilter::new("All files", Vec::<String>::new()),
                FileFilter::new("Rust files", ["rs"]),
            ],
            cx,
        )
    })
}

#[gpui_kit::test]
fn tilde_stands_for_the_home_and_the_path_filters_files(cx: &mut TestAppContext) {
    let setup = start(cx, project(), "~", keep());
    assert_eq!(
        setup.state.read_with(cx, |state, _| state.dir().clone()),
        sp("/home/me")
    );
    type_text(&setup, "main", cx);
    assert_eq!(names(&setup, cx), ["main.rs"]);
    press(&setup, "enter", cx);
    assert_eq!(confirmed(&setup), [[sp("/home/me/main.rs")]]);
}

#[gpui_kit::test]
fn escape_cancels_and_closes(cx: &mut TestAppContext) {
    let setup = start(cx, project(), "/home/me", keep());
    press(&setup, "escape", cx);
    assert_eq!(
        *setup.events.borrow().last().unwrap(),
        FilePickerEvent::Cancelled
    );
    assert!(!setup.open.get());
}

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
    let fake = Fake::default();
    let setup = start(cx, fake.clone(), "/home/me", keep());
    let (list, top) = (list_height(&setup, cx), footer_top(&setup, cx));
    let same = |setup: &Setup, cx: &mut TestAppContext, state: &str| {
        assert_eq!(list_height(setup, cx), list, "list height: {state}");
        assert_eq!(footer_top(setup, cx), top, "dialog height: {state}");
    };
    same(&setup, cx, "loading");

    let many: Vec<String> = (0..80).map(|n| format!("file-{n:02}.txt")).collect();
    let many: Vec<&str> = many.iter().map(String::as_str).collect();
    fake.resolve("/home/me", files(&many));
    settle(&setup, cx);
    same(&setup, cx, "eighty rows");

    type_text(&setup, "file-07", cx);
    same(&setup, cx, "one match");
    type_text(&setup, "zzzz", cx);
    same(&setup, cx, "no match");

    click(&setup, part("up"), cx);
    same(&setup, cx, "parent loading");
    fake.resolve("/home", Err(ListError::Other("denied".into())));
    settle(&setup, cx);
    same(&setup, cx, "failed");

    click(&setup, part("up"), cx);
    fake.resolve("/", files(&[]));
    settle(&setup, cx);
    same(&setup, cx, "empty");
}

#[gpui_kit::test]
fn the_local_disk_lists_folders_first_then_files(cx: &mut TestAppContext) {
    let dir = std::env::temp_dir().join(format!("gpui-cn-file-picker-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("Beta")).unwrap();
    std::fs::create_dir_all(dir.join("alpha")).unwrap();
    std::fs::write(dir.join("zeta.txt"), "12345").unwrap();
    std::fs::write(dir.join("Apple.txt"), "").unwrap();
    std::fs::write(dir.join(".hidden"), "").unwrap();
    cx.executor().allow_parking();
    let task =
        cx.update(|cx| LocalFiles.list(&PathStyle::host().path(dir.to_string_lossy()), None, cx));
    let page = cx.foreground_executor().block_test(task).unwrap();
    let listed: Vec<_> = page
        .entries()
        .iter()
        .map(|entry| (entry.name().to_string(), entry.is_folder(), entry.size()))
        .collect();
    assert_eq!(
        listed,
        [
            ("alpha".to_string(), true, None),
            ("Beta".to_string(), true, None),
            ("Apple.txt".to_string(), false, Some(0)),
            ("zeta.txt".to_string(), false, Some(5)),
            (".hidden".to_string(), false, Some(0)),
        ]
    );
    assert!(
        page.entries()
            .iter()
            .all(|entry| entry.modified().is_some())
    );
    let missing = cx.update(|cx| {
        LocalFiles.list(
            &PathStyle::host().path(dir.join("missing").to_string_lossy()),
            None,
            cx,
        )
    });
    assert!(cx.foreground_executor().block_test(missing).is_err());
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Waits for the real disk to answer, which is on another thread.
fn wait_for_rows(setup: &Setup, cx: &mut TestAppContext) {
    for _ in 0..400 {
        std::thread::sleep(std::time::Duration::from_millis(5));
        settle(setup, cx);
        if setup.state.read_with(cx, |state, _| state.row_count()) > 0 {
            return;
        }
    }
    panic!("the disk never answered");
}

#[gpui_kit::test]
fn on_the_real_disk_a_file_can_be_found_selected_and_chosen(cx: &mut TestAppContext) {
    let dir = std::env::temp_dir().join(format!("gpui-cn-file-picker-ui-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("a.rs"), "fn main() {}").unwrap();
    std::fs::write(dir.join("b.txt"), "notes").unwrap();
    std::fs::write(dir.join(".hidden"), "").unwrap();
    cx.executor().allow_parking();
    let setup = start(cx, LocalFiles, dir.to_str().unwrap(), filters());
    wait_for_rows(&setup, cx);

    assert_eq!(
        names(&setup, cx),
        ["sub", "a.rs", "b.txt"],
        "All files shows the files"
    );
    click(&setup, part("open"), cx);
    assert!(
        confirmed(&setup).is_empty(),
        "Open does nothing before a selection"
    );
    assert!(!present(&setup, part("status").into_child("show-all"), cx));
    assert!(present(
        &setup,
        part("status").into_child("show-hidden"),
        cx
    ));

    setup
        .state
        .update(cx, |state, cx| state.set_active_filter(1, cx));
    settle(&setup, cx);
    assert_eq!(names(&setup, cx), ["sub", "a.rs"]);
    assert!(
        present(&setup, part("status").into_child("show-all"), cx),
        "a hint says the filter hides b.txt"
    );
    click(&setup, part("status").into_child("show-all"), cx);
    assert_eq!(
        names(&setup, cx),
        ["sub", "a.rs", "b.txt"],
        "the action shows all"
    );

    click(&setup, entry("a.rs"), cx);
    assert_eq!(selection(&setup, cx), ["a.rs"]);
    click(&setup, part("open"), cx);
    assert_eq!(
        confirmed(&setup),
        [[PathStyle::host().path(dir.join("a.rs").to_string_lossy())]],
        "Open chooses the selection"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[gpui_kit::test]
fn a_folder_with_no_files_says_so_and_still_lists_its_folders(cx: &mut TestAppContext) {
    let setup = start(
        cx,
        MemoryFiles::new().with_dir("/x", [FileEntry::folder("only")]),
        "/x",
        filters(),
    );
    assert_eq!(names(&setup, cx), ["only"]);
    assert!(present(&setup, part("status"), cx));
    assert_eq!(setup.state.read_with(cx, |state, _| state.file_count()), 0);
}

trait IntoChild {
    fn into_child(self, name: &str) -> ElementId;
}

impl IntoChild for ElementId {
    fn into_child(self, name: &str) -> ElementId {
        ElementId::NamedChild(self.into(), name.to_string().into())
    }
}

#[gpui_kit::test]
fn a_shift_range_covers_the_rows_the_query_shows_and_no_others(cx: &mut TestAppContext) {
    let setup = start(cx, project(), "/home/me", multiple());
    type_text(&setup, "rs", cx);
    let shown = names(&setup, cx);
    assert!(shown.contains(&"lib.rs".to_string()) && shown.contains(&"main.rs".to_string()));
    assert!(
        !shown.contains(&"notes.txt".to_string()),
        "the query hides it"
    );
    let shift = Modifiers {
        shift: true,
        ..Modifiers::default()
    };
    let files: Vec<String> = shown
        .iter()
        .filter(|n| n.ends_with(".rs"))
        .cloned()
        .collect();
    click(&setup, entry(&files[0]), cx);
    click_with(&setup, entry(files.last().unwrap()), shift, cx);
    let mut expected = files.clone();
    expected.sort();
    let mut got = selection(&setup, cx);
    got.sort();
    assert_eq!(got, expected, "only the rows that show are in the range");
}
