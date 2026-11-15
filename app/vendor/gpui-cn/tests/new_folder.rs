//! "New folder" in both pickers: off by default, only when the source can
//! make folders, an inline row at the top of the list, the name checked
//! before the source is asked, the list read again from the source, and a
//! dialog that never changes size. Headless, over the fake remote, the
//! in-memory sources, and a real temporary directory.

mod support;

use std::time::Duration;

use gpui_cn::{
    CreateFolderError, FilePickerEvent, FolderPickerEvent, LocalFiles, LocalFolders, MemoryFolders,
    PathStyle,
};
use gpui_kit::{AppContext as _, TestAppContext};
use support::{
    FakeRemote,
    harness::{
        BOTH, NEW_FOLDER_KEY, OnlyFiles, OnlyFolders, Options, Picker, Setup, Which, bounds, click,
        dir, highlighted, list_bottom, list_height, part, present, press, row, settle, start, text,
        type_text, wait,
    },
};

fn allow() -> Options {
    Options {
        initial: Some("/home/me".into()),
        allow_new_folder: true,
        ..Options::default()
    }
}

fn button() -> gpui_kit::ElementId {
    part("new-folder-button")
}

fn row_id() -> gpui_kit::ElementId {
    part("new-folder")
}

fn error_id() -> gpui_kit::ElementId {
    part("new-folder-error")
}

/// The value of the field in the new folder row.
fn naming(setup: &Setup, cx: &mut TestAppContext) -> Option<String> {
    cx.update(|cx| match &setup.picker {
        Picker::Folder(state) => state
            .read(cx)
            .new_folder_name(cx)
            .map(|name| name.to_string()),
        Picker::File(state) => state
            .read(cx)
            .new_folder_name(cx)
            .map(|name| name.to_string()),
    })
}

fn created_events(setup: &Setup) -> Vec<String> {
    match setup.which {
        Which::Folder => setup
            .folder_events
            .borrow()
            .iter()
            .filter_map(|event| match event {
                FolderPickerEvent::FolderCreated(path) => Some(path.to_string()),
                _ => None,
            })
            .collect(),
        Which::File => setup
            .file_events
            .borrow()
            .iter()
            .filter_map(|event| match event {
                FilePickerEvent::FolderCreated(path) => Some(path.to_string()),
                _ => None,
            })
            .collect(),
    }
}

fn selected_dir_path(setup: &Setup, cx: &mut TestAppContext) -> Option<String> {
    cx.update(|cx| match &setup.picker {
        Picker::Folder(state) => state.read(cx).selected().map(|path| path.to_string()),
        Picker::File(_) => None,
    })
}

fn is_listed(setup: &Setup, name: &str, cx: &mut TestAppContext) -> bool {
    present(setup, row(name), cx)
}

#[gpui_kit::test]
fn the_button_is_off_by_default_and_needs_a_source_that_can_make_folders(cx: &mut TestAppContext) {
    for which in BOTH {
        let off = start(
            cx,
            which,
            FakeRemote::posix(),
            Options {
                initial: Some("/home/me".into()),
                ..Options::default()
            },
        );
        assert!(!present(&off, button(), cx), "{which:?}: off by default");
        let read_only = start(cx, which, FakeRemote::posix().read_only(), allow());
        assert!(
            !present(&read_only, button(), cx),
            "{which:?}: read-only source"
        );
        let on = start(cx, which, FakeRemote::posix(), allow());
        assert!(present(&on, button(), cx), "{which:?}: shown");
        cx.update(|cx| match &on.picker {
            Picker::Folder(state) => {
                assert!(state.read(cx).allows_new_folder());
                assert!(state.read(cx).can_create_folder());
            }
            Picker::File(state) => {
                assert!(state.read(cx).allows_new_folder());
                assert!(state.read(cx).can_create_folder());
            }
        });
    }
}

#[gpui_kit::test]
fn the_button_waits_for_the_listing(cx: &mut TestAppContext) {
    for which in BOTH {
        let remote = FakeRemote::posix().with_latency(Duration::from_millis(300));
        let setup = start(cx, which, remote, allow());
        assert!(present(&setup, button(), cx));
        click(&setup, button(), cx);
        assert!(!present(&setup, row_id(), cx), "{which:?}: still loading");
        wait(&setup, 400, cx);
        click(&setup, button(), cx);
        assert!(present(&setup, row_id(), cx), "{which:?}: listed");
    }
}

#[gpui_kit::test]
fn a_click_opens_the_row_with_the_default_name_selected(cx: &mut TestAppContext) {
    for which in BOTH {
        let remote = FakeRemote::posix();
        let setup = start(cx, which, remote.clone(), allow());
        assert!(!present(&setup, row_id(), cx));
        click(&setup, button(), cx);
        assert!(present(&setup, row_id(), cx), "{which:?}");
        assert_eq!(naming(&setup, cx).as_deref(), Some("Untitled folder"));
        type_text(&setup, "Projects", cx);
        assert_eq!(
            naming(&setup, cx).as_deref(),
            Some("Projects"),
            "{which:?}: the default name was selected, so typing replaces it"
        );
        assert!(
            remote.created().is_empty(),
            "nothing is made until confirmed"
        );
    }
}

#[gpui_kit::test]
fn the_keyboard_shortcut_opens_the_row(cx: &mut TestAppContext) {
    for which in BOTH {
        let setup = start(cx, which, FakeRemote::posix(), allow());
        press(&setup, NEW_FOLDER_KEY, cx);
        assert!(present(&setup, row_id(), cx), "{which:?}");
    }
}

#[gpui_kit::test]
fn enter_makes_the_folder_and_it_is_the_one_chosen(cx: &mut TestAppContext) {
    for which in BOTH {
        let remote = FakeRemote::posix();
        let setup = start(cx, which, remote.clone(), allow());
        click(&setup, button(), cx);
        type_text(&setup, "Projects", cx);
        press(&setup, "enter", cx);
        assert!(!present(&setup, row_id(), cx), "{which:?}: the row closed");
        assert_eq!(remote.created(), ["/home/me|Projects"]);
        assert!(remote.names("/home/me").contains(&"Projects".to_string()));
        assert!(is_listed(&setup, "Projects", cx), "{which:?}: listed");
        assert_eq!(
            remote.requested(),
            ["/home/me", "/home/me"],
            "{which:?}: the listing was read again, not guessed"
        );
        assert_eq!(created_events(&setup), ["/home/me/Projects"]);
        assert_eq!(highlighted(&setup, cx).as_deref(), Some("Projects"));
        match which {
            Which::Folder => {
                assert_eq!(
                    selected_dir_path(&setup, cx).as_deref(),
                    Some("/home/me/Projects"),
                    "Use folder chooses the new folder"
                );
                assert_eq!(text(&setup, cx), "/home/me/Projects");
            }
            Which::File => {
                assert_eq!(dir(&setup, cx), "/home/me", "the file picker stays");
            }
        }
    }
}

#[gpui_kit::test]
fn a_duplicate_name_shows_the_error_and_keeps_editing(cx: &mut TestAppContext) {
    for which in BOTH {
        let remote = FakeRemote::posix();
        let setup = start(cx, which, remote.clone(), allow());
        click(&setup, button(), cx);
        type_text(&setup, "docs", cx);
        press(&setup, "enter", cx);
        assert!(present(&setup, row_id(), cx), "{which:?}: still editing");
        assert!(present(&setup, error_id(), cx));
        assert_eq!(naming(&setup, cx).as_deref(), Some("docs"));
        assert!(
            remote.created().is_empty(),
            "{which:?}: the source was not asked"
        );
        type_text(&setup, "2", cx);
        assert!(
            !present(&setup, error_id(), cx),
            "{which:?}: the error goes on edit"
        );
        press(&setup, "enter", cx);
        assert_eq!(remote.created(), ["/home/me|docs2"]);
    }
}

#[gpui_kit::test]
fn the_source_can_refuse_and_the_row_shows_why(cx: &mut TestAppContext) {
    for which in BOTH {
        let remote = FakeRemote::posix();
        remote.fail_create(CreateFolderError::PermissionDenied);
        let setup = start(cx, which, remote.clone(), allow());
        click(&setup, button(), cx);
        press(&setup, "enter", cx);
        assert!(present(&setup, error_id(), cx), "{which:?}");
        assert!(present(&setup, row_id(), cx));
        assert_eq!(remote.created().len(), 1);
        press(&setup, "enter", cx);
        assert_eq!(remote.created().len(), 2, "{which:?}: edit and try again");
        assert!(
            !present(&setup, row_id(), cx),
            "{which:?}: made on the second try"
        );
        assert!(is_listed(&setup, "Untitled folder", cx));
    }
}

#[gpui_kit::test]
fn names_the_picker_refuses_never_reach_the_source(cx: &mut TestAppContext) {
    for which in BOTH {
        let remote = FakeRemote::posix();
        let setup = start(cx, which, remote.clone(), allow());
        click(&setup, button(), cx);
        for bad in ["a/b", "..", "a\\b"] {
            press(&setup, support::harness::SELECT_ALL, cx);
            type_text(&setup, bad, cx);
            press(&setup, "enter", cx);
            assert!(present(&setup, error_id(), cx), "{which:?}: {bad}");
            assert!(present(&setup, row_id(), cx));
        }
        press(&setup, support::harness::SELECT_ALL, cx);
        press(&setup, "backspace", cx);
        press(&setup, "enter", cx);
        assert!(present(&setup, error_id(), cx), "{which:?}: empty");
        assert!(
            remote.created().is_empty(),
            "{which:?}: the source was never asked"
        );
    }
}

#[gpui_kit::test]
fn escape_closes_the_row_and_leaves_the_dialog_open(cx: &mut TestAppContext) {
    for which in BOTH {
        let remote = FakeRemote::posix();
        let setup = start(cx, which, remote.clone(), allow());
        click(&setup, button(), cx);
        press(&setup, "escape", cx);
        assert!(!present(&setup, row_id(), cx), "{which:?}");
        assert!(setup.open.get(), "{which:?}: the dialog is still open");
        assert!(remote.created().is_empty());
        press(&setup, "escape", cx);
        assert!(
            !setup.open.get(),
            "{which:?}: a second Escape closes the dialog"
        );
    }
}

#[gpui_kit::test]
fn the_dialog_keeps_its_size_while_naming_creating_and_refusing(cx: &mut TestAppContext) {
    for which in BOTH {
        let remote = FakeRemote::posix().with_latency(Duration::from_millis(300));
        let setup = start(cx, which, remote.clone(), allow());
        wait(&setup, 400, cx);
        let height = list_height(&setup, cx);
        let bottom = list_bottom(&setup, cx);
        click(&setup, button(), cx);
        assert_eq!(list_height(&setup, cx), height, "{which:?}: row open");
        assert_eq!(list_bottom(&setup, cx), bottom);
        press(&setup, "enter", cx);
        assert!(
            present(&setup, part("new-folder-spinner"), cx),
            "{which:?}: waiting for the source"
        );
        assert_eq!(list_height(&setup, cx), height, "{which:?}: creating");
        assert_eq!(list_bottom(&setup, cx), bottom);
        wait(&setup, 400, cx);
        assert!(!present(&setup, row_id(), cx), "{which:?}: made");
        assert_eq!(list_height(&setup, cx), height, "{which:?}: listing again");
        wait(&setup, 400, cx);
        assert_eq!(list_height(&setup, cx), height, "{which:?}: done");
        assert_eq!(list_bottom(&setup, cx), bottom);
        remote.fail_create(CreateFolderError::Other("Disk full".into()));
        click(&setup, button(), cx);
        type_text(&setup, "again", cx);
        press(&setup, "enter", cx);
        wait(&setup, 400, cx);
        assert!(present(&setup, error_id(), cx), "{which:?}: refused");
        assert_eq!(list_height(&setup, cx), height, "{which:?}: error shown");
        assert_eq!(list_bottom(&setup, cx), bottom);
    }
}

#[gpui_kit::test]
fn a_row_that_is_being_made_ignores_input(cx: &mut TestAppContext) {
    for which in BOTH {
        let remote = FakeRemote::posix().with_latency(Duration::from_millis(300));
        let setup = start(cx, which, remote.clone(), allow());
        wait(&setup, 400, cx);
        click(&setup, button(), cx);
        press(&setup, "enter", cx);
        type_text(&setup, "x", cx);
        press(&setup, "enter", cx);
        press(&setup, "escape", cx);
        assert!(present(&setup, row_id(), cx), "{which:?}: Escape waits too");
        assert_eq!(remote.created().len(), 1, "{which:?}: asked once");
        wait(&setup, 400, cx);
        assert!(!present(&setup, row_id(), cx));
    }
}

#[gpui_kit::test]
fn the_tilde_resolves_to_the_remote_home_when_a_folder_is_made(cx: &mut TestAppContext) {
    for which in BOTH {
        let remote = FakeRemote::posix();
        let setup = start(
            cx,
            which,
            remote.clone(),
            Options {
                allow_new_folder: true,
                ..Options::default()
            },
        );
        assert_eq!(dir(&setup, cx), "/home/me");
        click(&setup, button(), cx);
        type_text(&setup, "fresh", cx);
        press(&setup, "enter", cx);
        assert_eq!(remote.created(), ["/home/me|fresh"], "{which:?}");
        assert_eq!(created_events(&setup), ["/home/me/fresh"]);
    }
}

#[gpui_kit::test]
fn a_windows_source_compares_names_without_case(cx: &mut TestAppContext) {
    for which in BOTH {
        let remote = FakeRemote::windows();
        let setup = start(
            cx,
            which,
            remote.clone(),
            Options {
                initial: Some("C:\\Users\\me".into()),
                allow_new_folder: true,
                ..Options::default()
            },
        );
        click(&setup, button(), cx);
        type_text(&setup, "documents", cx);
        press(&setup, "enter", cx);
        assert!(
            present(&setup, error_id(), cx),
            "{which:?}: Documents exists"
        );
        assert!(remote.created().is_empty());
        press(&setup, support::harness::SELECT_ALL, cx);
        type_text(&setup, "Photos", cx);
        press(&setup, "enter", cx);
        assert_eq!(remote.created(), ["C:\\Users\\me|Photos"]);
        assert_eq!(created_events(&setup), ["C:\\Users\\me\\Photos"]);
    }
}

#[gpui_kit::test]
fn the_row_closes_when_the_picker_moves_to_another_folder(cx: &mut TestAppContext) {
    for which in BOTH {
        let setup = start(cx, which, FakeRemote::posix(), allow());
        click(&setup, button(), cx);
        cx.update_window(setup.handle.into(), |_, window, cx| match &setup.picker {
            Picker::Folder(state) => state.update(cx, |state, cx| state.go_up(window, cx)),
            Picker::File(state) => state.update(cx, |state, cx| state.go_up(window, cx)),
        })
        .unwrap();
        settle(&setup, cx);
        assert!(!present(&setup, row_id(), cx), "{which:?}");
    }
}

#[gpui_kit::test]
fn the_programmatic_call_goes_through_the_same_row(cx: &mut TestAppContext) {
    for which in BOTH {
        let remote = FakeRemote::posix();
        let setup = start(cx, which, remote.clone(), allow());
        cx.update_window(setup.handle.into(), |_, window, cx| match &setup.picker {
            Picker::Folder(state) => state.update(cx, |state, cx| {
                state.create_folder("made", window, cx).unwrap()
            }),
            Picker::File(state) => state.update(cx, |state, cx| {
                state.create_folder("made", window, cx).unwrap()
            }),
        })
        .unwrap();
        settle(&setup, cx);
        assert_eq!(remote.created(), ["/home/me|made"], "{which:?}");
        assert_eq!(created_events(&setup), ["/home/me/made"]);
        assert!(!present(&setup, row_id(), cx));
    }
}

#[gpui_kit::test]
fn a_touch_button_has_the_touch_size(cx: &mut TestAppContext) {
    for which in BOTH {
        let setup = start(
            cx,
            which,
            FakeRemote::posix(),
            Options {
                touch: true,
                ..allow()
            },
        );
        let size = bounds(&setup, button(), cx).size;
        assert!(
            size.height >= gpui_kit::px(44.),
            "{which:?}: {:?} is a 44pt target",
            size.height
        );
        click(&setup, button(), cx);
        assert!(present(&setup, row_id(), cx), "{which:?}: a tap works");
    }
}

#[gpui_kit::test]
fn the_memory_sources_make_folders_that_list_and_open(cx: &mut TestAppContext) {
    let folders = MemoryFolders::new()
        .with_style(PathStyle::posix())
        .with_home("/home/me")
        .with_dir("/home/me", ["code"]);
    let setup = start(cx, Which::Folder, OnlyFolders(folders), allow());
    click(&setup, button(), cx);
    type_text(&setup, "new", cx);
    press(&setup, "enter", cx);
    assert!(is_listed(&setup, "new", cx));
    assert_eq!(created_events(&setup), ["/home/me/new"]);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        if let Picker::Folder(state) = &setup.picker {
            state.update(cx, |state, cx| state.go_up(window, cx));
        }
    })
    .unwrap();
    settle(&setup, cx);
    assert_eq!(dir(&setup, cx), "/home");
}

/// A temporary directory that is removed when the test ends, even when an
/// assertion fails.
struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "gpui-cn-new-folder-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

fn listed(setup: &Setup, cx: &mut TestAppContext) -> bool {
    cx.update(|cx| match &setup.picker {
        Picker::Folder(state) => state
            .read(cx)
            .listing()
            .is_some_and(|l| matches!(l, gpui_cn::Listing::Ready(_))),
        Picker::File(state) => state
            .read(cx)
            .listing()
            .is_some_and(|l| matches!(l, gpui_cn::FileListing::Ready(_))),
    })
}

fn make_on_disk<S: gpui_cn::FolderSource + gpui_cn::FileSource + Clone>(
    cx: &mut TestAppContext,
    which: Which,
    source: S,
    tag: &str,
) {
    cx.executor().allow_parking();
    let root = TempDir::new(tag);
    let setup = start(
        cx,
        which,
        source,
        Options {
            initial: Some(root.path().to_string_lossy().into_owned()),
            allow_new_folder: true,
            ..Options::default()
        },
    );
    for _ in 0..250 {
        settle(&setup, cx);
        if listed(&setup, cx) {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(listed(&setup, cx), "the directory listed");
    click(&setup, button(), cx);
    type_text(&setup, "made-here", cx);
    press(&setup, "enter", cx);
    for _ in 0..250 {
        settle(&setup, cx);
        if root.path().join("made-here").is_dir() && created_events(&setup).len() == 1 {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        root.path().join("made-here").is_dir(),
        "{which:?}: the folder exists on disk"
    );
    assert_eq!(created_events(&setup).len(), 1, "{which:?}");
    for _ in 0..250 {
        settle(&setup, cx);
        if listed(&setup, cx) && present(&setup, button(), cx) {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    click(&setup, button(), cx);
    type_text(&setup, "made-here", cx);
    press(&setup, "enter", cx);
    assert!(present(&setup, error_id(), cx), "{which:?}: it exists now");
}

#[gpui_kit::test]
fn the_local_folder_source_makes_a_real_folder_on_disk(cx: &mut TestAppContext) {
    make_on_disk(cx, Which::Folder, OnlyFolders(LocalFolders), "folders");
}

#[gpui_kit::test]
fn the_local_file_source_makes_a_real_folder_on_disk(cx: &mut TestAppContext) {
    make_on_disk(cx, Which::File, OnlyFiles(LocalFiles), "files");
}
