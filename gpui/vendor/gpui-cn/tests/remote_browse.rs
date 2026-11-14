//! Browsing a remote in its own path style: drive switching on a Windows
//! style, paging, the remote's separator in typed paths and in what is
//! chosen, going up to the remote's root, and `~` from the remote's home.
//! Both pickers, over the fake remote.

mod support;

use gpui_cn::{FilePickerEvent, FolderPickerEvent};
use gpui_kit::{AppContext as _, TestAppContext};
use support::{
    FakeRemote,
    harness::{
        BOTH, Options, Picker, SELECT_ALL, Setup, Which, click, dir, highlighted, part, present,
        press, row, settle, start, text, type_text,
    },
};

fn initial(path: &str) -> Options {
    Options {
        initial: Some(path.into()),
        ..Options::default()
    }
}

fn clear(setup: &Setup, cx: &mut TestAppContext) {
    press(setup, SELECT_ALL, cx);
    press(setup, "backspace", cx);
}

fn go_up(setup: &Setup, cx: &mut TestAppContext) {
    cx.update_window(setup.handle.into(), |_, window, cx| match &setup.picker {
        Picker::Folder(state) => state.update(cx, |state, cx| state.go_up(window, cx)),
        Picker::File(state) => state.update(cx, |state, cx| state.go_up(window, cx)),
    })
    .unwrap();
    settle(setup, cx);
}

#[gpui_kit::test]
fn typing_a_drive_letter_switches_drives(cx: &mut TestAppContext) {
    for which in BOTH {
        let remote = FakeRemote::windows();
        let setup = start(cx, which, remote.clone(), initial("C:\\Users\\me"));
        assert_eq!(dir(&setup, cx), "C:\\Users\\me");
        clear(&setup, cx);
        type_text(&setup, "D:", cx);
        assert_eq!(
            dir(&setup, cx),
            "D:\\",
            "{which:?}: a lone drive is its root"
        );
        assert!(present(&setup, row("data"), cx), "{which:?}");
        clear(&setup, cx);
        type_text(&setup, "c:/users/ME/code/", cx);
        assert_eq!(dir(&setup, cx).to_lowercase(), "c:\\users\\me\\code");
        assert!(
            present(&setup, row("main.rs"), cx) || which == Which::Folder,
            "{which:?}: listed with the other spelling"
        );
    }
}

#[gpui_kit::test]
fn a_windows_listing_sorts_and_filters_without_case(cx: &mut TestAppContext) {
    for which in BOTH {
        let remote = FakeRemote::windows();
        let setup = start(cx, which, remote, initial("C:\\Users\\me"));
        clear(&setup, cx);
        type_text(&setup, "C:\\Users\\me\\doc", cx);
        assert!(present(&setup, row("Documents"), cx), "{which:?}");
        assert!(!present(&setup, row("Code"), cx));
        assert_eq!(highlighted(&setup, cx).as_deref(), Some("Documents"));
    }
}

#[gpui_kit::test]
fn the_separator_the_user_types_is_the_one_a_folder_is_entered_with(cx: &mut TestAppContext) {
    for which in BOTH {
        let remote = FakeRemote::windows();
        let setup = start(cx, which, remote, initial("C:/Users/me/"));
        type_text(&setup, "cod", cx);
        type_text(&setup, "/", cx);
        assert_eq!(
            text(&setup, cx),
            "~/Code/",
            "{which:?}: under the remote home, with the separator typed"
        );
        assert_eq!(dir(&setup, cx), "C:\\Users\\me\\Code");
        let setup = start(cx, which, FakeRemote::windows(), initial("C:\\Users\\me\\"));
        type_text(&setup, "cod", cx);
        type_text(&setup, "\\", cx);
        assert_eq!(text(&setup, cx), "~\\Code\\", "{which:?}");
    }
}

#[gpui_kit::test]
fn up_goes_to_the_remote_root_and_stays_there(cx: &mut TestAppContext) {
    for which in BOTH {
        let setup = start(cx, which, FakeRemote::posix(), initial("/home/me/code"));
        for expected in ["/home/me", "/home", "/", "/"] {
            go_up(&setup, cx);
            assert_eq!(dir(&setup, cx), expected, "{which:?}");
        }
        assert!(
            present(&setup, row("home"), cx),
            "{which:?}: the root lists"
        );
        let setup = start(cx, which, FakeRemote::windows(), initial("C:\\Users\\me"));
        for expected in ["C:\\Users", "C:\\", "C:\\"] {
            go_up(&setup, cx);
            assert_eq!(dir(&setup, cx), expected, "{which:?}");
        }
    }
}

#[gpui_kit::test]
fn a_big_listing_pages_and_the_picker_loads_the_next_page_on_a_click(cx: &mut TestAppContext) {
    for which in BOTH {
        let remote = FakeRemote::posix().with_page_size(2);
        let setup = start(cx, which, remote.clone(), initial("/home/me"));
        assert!(present(&setup, row("code"), cx), "{which:?}: first page");
        assert!(!present(&setup, row("docs"), cx), "{which:?}: not yet");
        assert!(
            present(&setup, part("more"), cx),
            "{which:?}: a Load more row"
        );
        click(&setup, part("more"), cx);
        assert!(present(&setup, row("docs"), cx), "{which:?}: second page");
        assert_eq!(
            remote.requested(),
            ["/home/me", "/home/me@2"],
            "{which:?}: the second request carries the token"
        );
    }
}

#[gpui_kit::test]
fn what_is_chosen_is_written_with_the_remotes_separator(cx: &mut TestAppContext) {
    let setup = start(
        cx,
        Which::Folder,
        FakeRemote::windows(),
        initial("C:\\Users\\me"),
    );
    clear(&setup, cx);
    type_text(&setup, "C:/Users/me/documents", cx);
    cx.update(|cx| {
        let Picker::Folder(state) = &setup.picker else {
            unreachable!()
        };
        state.update(cx, |state, cx| state.choose(cx));
    });
    settle(&setup, cx);
    let events = setup.folder_events.borrow().clone();
    let [FolderPickerEvent::Chosen(path)] = events.as_slice() else {
        panic!("one choice, got {events:?}");
    };
    assert_eq!(path.as_str(), "C:\\Users\\me\\Documents");

    let setup = start(
        cx,
        Which::File,
        FakeRemote::windows(),
        initial("C:/Users/me/"),
    );
    click(&setup, row("notes.txt"), cx);
    cx.update(|cx| {
        let Picker::File(state) = &setup.picker else {
            unreachable!()
        };
        state.update(cx, |state, cx| state.confirm(cx));
    });
    settle(&setup, cx);
    let events = setup.file_events.borrow().clone();
    let confirmed: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            FilePickerEvent::Confirmed(paths) => {
                Some(paths.iter().map(|p| p.to_string()).collect::<Vec<_>>())
            }
            _ => None,
        })
        .collect();
    assert_eq!(confirmed, [vec!["C:\\Users\\me\\notes.txt".to_string()]]);
}

#[gpui_kit::test]
fn the_remotes_home_is_the_start_whatever_the_host_is(cx: &mut TestAppContext) {
    for which in BOTH {
        let setup = start(cx, which, FakeRemote::posix(), Options::default());
        assert_eq!(text(&setup, cx), "~/");
        assert_eq!(dir(&setup, cx), "/home/me", "{which:?}");
        let setup = start(cx, which, FakeRemote::windows(), Options::default());
        assert_eq!(text(&setup, cx), "~\\");
        assert_eq!(dir(&setup, cx), "C:\\Users\\me", "{which:?}");
        clear(&setup, cx);
        type_text(&setup, "~/Code/", cx);
        assert_eq!(dir(&setup, cx), "C:\\Users\\me\\Code", "{which:?}");
    }
}
