//! Regression tests for the picker defects found in review: scroll place
//! across pages, hostile names, `..` in paths, the New folder row, and the
//! errors of later pages.

mod support;

use gpui_cn::ListError;
use gpui_kit::{AppContext as _, ScrollDelta, TestAppContext, point, px, test::TestWindowExt as _};
use support::{
    FakeRemote, folder,
    harness::{
        BOTH, Options, Picker, SELECT_ALL, Setup, Which, bounds, click, dir, part, present, press,
        row, settle, start, type_text,
    },
};

fn at(path: &str) -> Options {
    Options {
        initial: Some(path.into()),
        ..Options::default()
    }
}

fn big(page: usize) -> FakeRemote {
    FakeRemote::posix()
        .with_dir("/big", (0..60).map(|n| folder(&format!("d{n:02}"))))
        .with_page_size(page)
}

fn load_more(setup: &Setup, cx: &mut TestAppContext) {
    cx.update(|cx| match &setup.picker {
        Picker::Folder(state) => state.update(cx, |state, cx| state.load_more(cx)),
        Picker::File(state) => state.update(cx, |state, cx| state.load_more(cx)),
    });
    settle(setup, cx);
}

fn first_visible(setup: &Setup, cx: &mut TestAppContext) -> (String, gpui_kit::Pixels) {
    let list_top = bounds(setup, part("list"), cx).top();
    for n in 0..60 {
        let name = format!("d{n:02}");
        if present(setup, row(&name), cx) {
            let top = bounds(setup, row(&name), cx).top();
            if top >= list_top {
                return (name, top);
            }
        }
    }
    panic!("no row is visible");
}

fn scroll_down(setup: &Setup, cx: &mut TestAppContext) {
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.scroll(
            part("rows"),
            ScrollDelta::Pixels(point(px(0.), px(-300.))),
            cx,
        );
    })
    .unwrap();
    settle(setup, cx);
}

#[gpui_kit::test]
fn loading_a_page_keeps_the_scroll_place(cx: &mut TestAppContext) {
    for which in BOTH {
        let remote = big(20);
        let setup = start(cx, which, remote, at("/big"));
        load_more(&setup, cx);
        scroll_down(&setup, cx);
        let (name, top) = first_visible(&setup, cx);
        assert_ne!(name, "d00", "{which:?}: the list scrolled");
        load_more(&setup, cx);
        assert!(
            present(&setup, row(&name), cx),
            "{which:?}: {name} is still on screen"
        );
        assert_eq!(
            bounds(&setup, row(&name), cx).top(),
            top,
            "{which:?}: {name} stays where it was"
        );
    }
}

#[gpui_kit::test]
fn loading_a_page_keeps_the_scroll_place_under_a_query(cx: &mut TestAppContext) {
    for which in BOTH {
        let setup = start(cx, which, big(20), at("/big"));
        type_text(&setup, "d", cx);
        load_more(&setup, cx);
        scroll_down(&setup, cx);
        let (name, top) = first_visible(&setup, cx);
        assert_ne!(name, "d00", "{which:?}: the list scrolled");
        load_more(&setup, cx);
        assert!(
            present(&setup, row(&name), cx),
            "{which:?}: {name} is still on screen"
        );
        assert_eq!(
            bounds(&setup, row(&name), cx).top(),
            top,
            "{which:?}: {name} stays where it was"
        );
    }
}

fn rows_of(setup: &Setup, cx: &mut TestAppContext) -> Vec<String> {
    cx.update(|cx| match &setup.picker {
        Picker::Folder(state) => state
            .read(cx)
            .listing()
            .map(|listing| match listing {
                gpui_cn::Listing::Ready(loaded) => loaded
                    .entries()
                    .iter()
                    .map(|entry| entry.name().to_string())
                    .collect(),
                _ => Vec::new(),
            })
            .unwrap_or_default(),
        Picker::File(state) => state
            .read(cx)
            .names()
            .iter()
            .map(|n| n.to_string())
            .collect(),
    })
}

#[gpui_kit::test]
fn a_hostile_listing_never_shows_names_that_climb_or_nest(cx: &mut TestAppContext) {
    for which in BOTH {
        let posix = FakeRemote::posix().with_dir(
            "/evil",
            [
                folder(".."),
                folder("."),
                folder("a/b"),
                folder("../../x"),
                folder("nul\0byte"),
                folder("tab\tname"),
                folder(""),
                folder("ok"),
                folder("back\\slash"),
            ],
        );
        let setup = start(cx, which, posix, at("/evil"));
        let mut names = rows_of(&setup, cx);
        names.sort();
        assert_eq!(
            names,
            ["back\\slash", "ok"],
            "{which:?}: a backslash is a name character on a POSIX style"
        );
        let windows = FakeRemote::windows().with_dir(
            "C:\\evil",
            [
                folder(".."),
                folder("a/b"),
                folder("a\\..\\..\\x"),
                folder("../../x"),
                folder("nul\0byte"),
                folder("fine"),
            ],
        );
        let setup = start(cx, which, windows, at("C:\\evil"));
        assert_eq!(rows_of(&setup, cx), ["fine"], "{which:?}: Windows style");
    }
}

#[gpui_kit::test]
fn dots_in_a_typed_path_fold_and_the_chosen_path_never_ends_in_them(cx: &mut TestAppContext) {
    for which in BOTH {
        let setup = start(cx, which, FakeRemote::posix(), at("/home/me/"));
        press(&setup, SELECT_ALL, cx);
        type_text(&setup, "/home/me/..", cx);
        assert_eq!(dir(&setup, cx), "/home", "{which:?}: .. lists the parent");
        press(&setup, SELECT_ALL, cx);
        type_text(&setup, "/home/me/../", cx);
        assert_eq!(dir(&setup, cx), "/home");
        press(&setup, SELECT_ALL, cx);
        type_text(&setup, "/..", cx);
        assert_eq!(dir(&setup, cx), "/", "{which:?}: never above the root");
        if which == Which::Folder {
            press(&setup, SELECT_ALL, cx);
            type_text(&setup, "/home/me/..", cx);
            cx.update(|cx| {
                if let Picker::Folder(state) = &setup.picker {
                    assert_eq!(state.read(cx).selected().unwrap().as_str(), "/home");
                }
            });
        }
        let setup = start(cx, which, FakeRemote::windows(), at("C:\\Users\\me"));
        press(&setup, SELECT_ALL, cx);
        type_text(&setup, "C:\\Users\\me\\..\\..\\..", cx);
        assert_eq!(dir(&setup, cx), "C:\\", "{which:?}: Windows style");
    }
}

fn new_folder_options(path: &str) -> Options {
    Options {
        initial: Some(path.into()),
        allow_new_folder: true,
        ..Options::default()
    }
}

fn create(
    setup: &Setup,
    name: &str,
    cx: &mut TestAppContext,
) -> Result<(), gpui_cn::CreateFolderError> {
    let result = cx
        .update_window(setup.handle.into(), |_, window, cx| match &setup.picker {
            Picker::Folder(state) => {
                state.update(cx, |state, cx| state.create_folder(name, window, cx))
            }
            Picker::File(state) => {
                state.update(cx, |state, cx| state.create_folder(name, window, cx))
            }
        })
        .unwrap();
    settle(setup, cx);
    result
}

#[gpui_kit::test]
fn create_folder_uses_the_name_it_is_given_when_the_row_is_open(cx: &mut TestAppContext) {
    for which in BOTH {
        let remote = FakeRemote::posix();
        let setup = start(cx, which, remote.clone(), new_folder_options("/home/me"));
        click(&setup, part("new-folder-button"), cx);
        type_text(&setup, "typed", cx);
        assert_eq!(create(&setup, "explicit", cx), Ok(()));
        assert_eq!(remote.created(), ["/home/me|explicit"], "{which:?}");
    }
}

#[gpui_kit::test]
fn create_folder_says_why_it_cannot_run(cx: &mut TestAppContext) {
    for which in BOTH {
        let remote = FakeRemote::posix().with_latency(std::time::Duration::from_millis(300));
        let setup = start(cx, which, remote.clone(), new_folder_options("/home/me"));
        assert_eq!(
            create(&setup, "x", cx),
            Err(gpui_cn::CreateFolderError::NotReady),
            "{which:?}: still loading"
        );
        assert!(remote.created().is_empty());
        let off = start(cx, which, FakeRemote::posix(), at("/home/me"));
        assert_eq!(
            create(&off, "x", cx),
            Err(gpui_cn::CreateFolderError::Unsupported),
            "{which:?}: not allowed"
        );
        let ready = start(
            cx,
            which,
            FakeRemote::posix(),
            new_folder_options("/home/me"),
        );
        assert!(matches!(
            create(&ready, "a/b", cx),
            Err(gpui_cn::CreateFolderError::InvalidName(_))
        ));
        assert!(
            present(&ready, part("new-folder"), cx),
            "{which:?}: the row shows why"
        );
        assert_eq!(
            create(&ready, "docs", cx),
            Err(gpui_cn::CreateFolderError::Exists)
        );
    }
}

#[gpui_kit::test]
fn a_folder_made_after_the_user_left_does_not_move_the_picker(cx: &mut TestAppContext) {
    for which in BOTH {
        let remote = FakeRemote::posix();
        let setup = start(cx, which, remote.clone(), new_folder_options("/home/me"));
        remote
            .shared
            .latency
            .set(std::time::Duration::from_millis(300));
        click(&setup, part("new-folder-button"), cx);
        type_text(&setup, "slow", cx);
        press(&setup, "enter", cx);
        cx.update_window(setup.handle.into(), |_, window, cx| match &setup.picker {
            Picker::Folder(state) => state.update(cx, |state, cx| state.go_up(window, cx)),
            Picker::File(state) => state.update(cx, |state, cx| state.go_up(window, cx)),
        })
        .unwrap();
        support::harness::wait(&setup, 400, cx);
        support::harness::wait(&setup, 400, cx);
        assert_eq!(
            dir(&setup, cx),
            "/home",
            "{which:?}: still where the user went"
        );
        assert_eq!(
            support::harness::text(&setup, cx),
            "/home/",
            "{which:?}: the field is not rewritten"
        );
        assert_ne!(
            support::harness::highlighted(&setup, cx).as_deref(),
            Some("slow"),
            "{which:?}: no highlight jump"
        );
        let made = match which {
            Which::Folder => setup
                .folder_events
                .borrow()
                .iter()
                .filter_map(|e| match e {
                    gpui_cn::FolderPickerEvent::FolderCreated(p) => Some(p.to_string()),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            Which::File => setup
                .file_events
                .borrow()
                .iter()
                .filter_map(|e| match e {
                    gpui_cn::FilePickerEvent::FolderCreated(p) => Some(p.to_string()),
                    _ => None,
                })
                .collect::<Vec<_>>(),
        };
        assert_eq!(made, ["/home/me/slow"], "{which:?}");
    }
}

fn selection_events(setup: &Setup) -> usize {
    setup
        .file_events
        .borrow()
        .iter()
        .filter(|e| matches!(e, gpui_cn::FilePickerEvent::SelectionChanged))
        .count()
}

fn selection(setup: &Setup, cx: &mut TestAppContext) -> Vec<String> {
    cx.update(|cx| match &setup.picker {
        Picker::File(state) => state
            .read(cx)
            .selection()
            .iter()
            .map(|p| p.to_string())
            .collect(),
        Picker::Folder(_) => Vec::new(),
    })
}

#[gpui_kit::test]
fn making_a_folder_keeps_the_file_selection(cx: &mut TestAppContext) {
    let setup = start(
        cx,
        Which::File,
        FakeRemote::posix(),
        new_folder_options("/home/me"),
    );
    click(&setup, row("notes.txt"), cx);
    assert_eq!(selection(&setup, cx), ["/home/me/notes.txt"]);
    click(&setup, part("new-folder-button"), cx);
    type_text(&setup, "fresh", cx);
    press(&setup, "enter", cx);
    assert_eq!(
        selection(&setup, cx),
        ["/home/me/notes.txt"],
        "the selection stays"
    );
}

#[gpui_kit::test]
fn leaving_the_folder_reports_that_the_selection_changed(cx: &mut TestAppContext) {
    let setup = start(cx, Which::File, FakeRemote::posix(), at("/home/me"));
    click(&setup, row("notes.txt"), cx);
    let before = selection_events(&setup);
    cx.update_window(setup.handle.into(), |_, window, cx| {
        if let Picker::File(state) = &setup.picker {
            state.update(cx, |state, cx| state.go_up(window, cx));
        }
    })
    .unwrap();
    settle(&setup, cx);
    assert!(selection(&setup, cx).is_empty());
    assert_eq!(
        selection_events(&setup),
        before + 1,
        "going up reported the change"
    );
}

#[gpui_kit::test]
fn a_folder_named_more_is_a_row_not_the_load_more_row(cx: &mut TestAppContext) {
    let remote = FakeRemote::posix()
        .with_dir("/m", [folder("more"), folder("other")])
        .with_page_size(1);
    let setup = start(cx, Which::Folder, remote, at("/m"));
    assert!(present(&setup, row("more"), cx), "the folder row");
    assert!(present(&setup, part("more"), cx), "the Load more row");
    click(&setup, part("more"), cx);
    assert!(
        present(&setup, row("other"), cx),
        "Load more loaded the page"
    );
    click(&setup, row("more"), cx);
    assert_eq!(dir(&setup, cx), "/m/more", "the folder opened");
}

#[gpui_kit::test]
fn a_later_page_that_needs_a_login_keeps_its_kind(cx: &mut TestAppContext) {
    for which in BOTH {
        let remote = FakeRemote::posix().with_page_size(2);
        let setup = start(
            cx,
            which,
            remote.clone(),
            Options {
                auth: true,
                ..at("/home/me")
            },
        );
        remote.fail_next(ListError::AuthRequired("Sign in again".into()));
        load_more(&setup, cx);
        click(&setup, part("more"), cx);
        assert_eq!(setup.signed_in.get(), 1, "{which:?}: the row signs in");
        assert_eq!(remote.requested().len(), 2, "{which:?}: no blind retry");
        let remote = FakeRemote::posix().with_page_size(2);
        let setup = start(cx, which, remote.clone(), at("/home/me"));
        remote.fail_next(ListError::Disconnected("Lost".into()));
        load_more(&setup, cx);
        click(&setup, part("more"), cx);
        assert_eq!(
            remote.requested().len(),
            3,
            "{which:?}: a lost connection retries"
        );
    }
}

#[gpui_kit::test]
fn choosing_cancels_the_requests_still_running(cx: &mut TestAppContext) {
    for which in BOTH {
        let remote = FakeRemote::posix().with_page_size(4);
        let setup = start(cx, which, remote.clone(), at("/home/me"));
        if which == Which::File {
            click(&setup, row("notes.txt"), cx);
        }
        remote
            .shared
            .latency
            .set(std::time::Duration::from_millis(300));
        cx.update(|cx| match &setup.picker {
            Picker::Folder(state) => state.update(cx, |state, cx| state.load_more(cx)),
            Picker::File(state) => state.update(cx, |state, cx| state.load_more(cx)),
        });
        settle(&setup, cx);
        assert_eq!(remote.cancelled(), 0);
        cx.update(|cx| match &setup.picker {
            Picker::Folder(state) => state.update(cx, |state, cx| state.choose(cx)),
            Picker::File(state) => state.update(cx, |state, cx| state.confirm(cx)),
        });
        settle(&setup, cx);
        assert_eq!(
            remote.cancelled(),
            1,
            "{which:?}: the page request was dropped"
        );
    }
}

#[gpui_kit::test]
fn select_all_in_the_new_folder_name_selects_the_name_not_the_files(cx: &mut TestAppContext) {
    let setup = start(
        cx,
        Which::File,
        FakeRemote::posix(),
        Options {
            multiple: true,
            ..new_folder_options("/home/me")
        },
    );
    click(&setup, part("new-folder-button"), cx);
    type_text(&setup, "abc", cx);
    press(&setup, SELECT_ALL, cx);
    assert!(selection(&setup, cx).is_empty(), "no file was selected");
    type_text(&setup, "Z", cx);
    assert_eq!(
        support::harness::text(&setup, cx),
        "/home/me/",
        "the path field is untouched"
    );
    cx.update(|cx| {
        if let Picker::File(state) = &setup.picker {
            assert_eq!(state.read(cx).new_folder_name(cx).as_deref(), Some("Z"));
        }
    });
}
