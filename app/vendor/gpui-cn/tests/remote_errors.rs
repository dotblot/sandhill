//! A slow or failing remote shows the list's own loading and error states
//! without moving the dialog, never blocks rendering, and is cancelled when
//! the dialog closes. Both pickers, over the fake remote.

mod support;

use std::time::Duration;

use gpui_cn::ListError;
use gpui_kit::{AppContext as _, TestAppContext};
use support::{
    FakeRemote,
    harness::{
        BOTH, Options, Picker, Setup, Which, cancel, click, list_bottom, list_height, part,
        present, retry, row, settle, start, wait,
    },
};

fn opts() -> Options {
    Options {
        initial: Some("/home/me".into()),
        ..Options::default()
    }
}

fn failure(setup: &Setup, cx: &mut TestAppContext) -> Option<ListError> {
    cx.update(|cx| match &setup.picker {
        Picker::Folder(state) => state.read(cx).failure().cloned(),
        Picker::File(state) => state.read(cx).failure().cloned(),
    })
}

#[gpui_kit::test]
fn a_lost_connection_shows_its_message_and_retry_lists_again(cx: &mut TestAppContext) {
    for which in BOTH {
        let remote = FakeRemote::posix();
        remote.fail_next(ListError::Disconnected(
            "Connection to the server was lost".into(),
        ));
        let setup = start(cx, which, remote.clone(), opts());
        assert!(present(&setup, part("message"), cx), "{which:?}");
        assert!(present(&setup, part("retry"), cx), "{which:?}");
        assert!(!present(&setup, part("sign-in"), cx));
        assert_eq!(
            failure(&setup, cx),
            Some(ListError::Disconnected(
                "Connection to the server was lost".into()
            ))
        );
        click(&setup, part("retry"), cx);
        assert!(
            !present(&setup, part("message"), cx),
            "{which:?}: recovered"
        );
        assert!(present(&setup, row("code"), cx), "{which:?}: rows");
        assert_eq!(
            remote.requested(),
            ["/home/me", "/home/me"],
            "{which:?}: one request, then one more for the retry"
        );
    }
}

#[gpui_kit::test]
fn a_missing_folder_has_a_message_and_no_action(cx: &mut TestAppContext) {
    let setup = start(cx, Which::Folder, FakeRemote::posix(), opts());
    support::harness::press(&setup, support::harness::SELECT_ALL, cx);
    support::harness::press(&setup, "backspace", cx);
    support::harness::type_text(&setup, "/nowhere/", cx);
    assert_eq!(failure(&setup, cx), Some(ListError::NotFound));
    assert!(present(&setup, part("message"), cx));
    assert!(!present(&setup, part("retry"), cx));
    assert!(!present(&setup, part("sign-in"), cx));
}

#[gpui_kit::test]
fn a_source_that_needs_a_login_shows_sign_in_and_the_app_decides_the_rest(cx: &mut TestAppContext) {
    for which in BOTH {
        let remote = FakeRemote::posix().requiring_sign_in();
        let signing = remote.clone();
        let setup = start(
            cx,
            which,
            remote.clone(),
            Options {
                auth: true,
                sign_in: Some(std::rc::Rc::new(move || signing.sign_in())),
                ..opts()
            },
        );
        assert!(present(&setup, part("sign-in"), cx), "{which:?}");
        assert!(!present(&setup, part("retry"), cx));
        assert!(matches!(
            failure(&setup, cx),
            Some(ListError::AuthRequired(_))
        ));
        click(&setup, part("sign-in"), cx);
        assert_eq!(setup.signed_in.get(), 1, "the app's handler ran");
        assert!(
            present(&setup, part("message"), cx),
            "{which:?}: nothing is listed again until the app says so"
        );
        retry(&setup, cx);
        assert!(!present(&setup, part("message"), cx), "{which:?}");
        assert!(
            present(&setup, row("code"), cx),
            "{which:?}: rows after the login"
        );
        assert_eq!(remote.requested().len(), 2);
    }
}

#[gpui_kit::test]
fn without_a_handler_there_is_no_sign_in_button(cx: &mut TestAppContext) {
    let setup = start(
        cx,
        Which::Folder,
        FakeRemote::posix().requiring_sign_in(),
        opts(),
    );
    assert!(present(&setup, part("message"), cx));
    assert!(!present(&setup, part("sign-in"), cx));
}

#[gpui_kit::test]
fn a_slow_remote_shows_the_spinner_and_the_dialog_never_moves(cx: &mut TestAppContext) {
    for which in BOTH {
        let remote = FakeRemote::posix().with_latency(Duration::from_millis(400));
        remote.fail_next(ListError::Other("The server said no".into()));
        let setup = start(cx, which, remote, opts());
        assert!(
            present(&setup, part("loading"), cx),
            "{which:?}: still loading"
        );
        let height = list_height(&setup, cx);
        let bottom = list_bottom(&setup, cx);
        wait(&setup, 500, cx);
        assert!(present(&setup, part("message"), cx), "{which:?}: failed");
        assert_eq!(list_height(&setup, cx), height, "{which:?}: error");
        assert_eq!(list_bottom(&setup, cx), bottom);
        click(&setup, part("retry"), cx);
        assert!(
            present(&setup, part("loading"), cx),
            "{which:?}: loading again"
        );
        assert_eq!(list_height(&setup, cx), height, "{which:?}: loading again");
        wait(&setup, 500, cx);
        assert!(present(&setup, row("code"), cx), "{which:?}: listed");
        assert_eq!(list_height(&setup, cx), height, "{which:?}: rows");
        assert_eq!(list_bottom(&setup, cx), bottom);
    }
}

#[gpui_kit::test]
fn closing_the_dialog_mid_list_cancels_the_request(cx: &mut TestAppContext) {
    for which in BOTH {
        let remote = FakeRemote::posix().with_latency(Duration::from_millis(400));
        let setup = start(cx, which, remote.clone(), opts());
        assert_eq!(remote.requested(), ["/home/me"]);
        assert_eq!(remote.cancelled(), 0);
        cancel(&setup, cx);
        assert_eq!(remote.cancelled(), 1, "{which:?}: the request was dropped");
        wait(&setup, 1000, cx);
        assert_eq!(
            remote.shared.answered.get(),
            0,
            "{which:?}: nothing answered"
        );
        let events = match which {
            Which::Folder => setup.folder_events.borrow().len(),
            Which::File => setup.file_events.borrow().len(),
        };
        assert_eq!(events, 1, "{which:?}: only the cancel itself is reported");
        setup.open.set(true);
        settle(&setup, cx);
        assert!(present(&setup, part("loading"), cx), "{which:?}: reopened");
        assert_eq!(remote.requested(), ["/home/me", "/home/me"], "listed again");
        wait(&setup, 500, cx);
        assert!(present(&setup, row("code"), cx), "{which:?}: rows");
    }
}

#[gpui_kit::test]
fn leaving_a_slow_folder_cancels_its_request(cx: &mut TestAppContext) {
    let remote = FakeRemote::posix();
    let setup = start(cx, Which::Folder, remote.clone(), opts());
    remote.shared.latency.set(Duration::from_millis(400));
    let Picker::Folder(state) = &setup.picker else {
        unreachable!()
    };
    cx.update_window(setup.handle.into(), |_, window, cx| {
        state.update(cx, |state, cx| state.descend("code", window, cx));
    })
    .unwrap();
    settle(&setup, cx);
    assert_eq!(
        remote.requested().last().map(String::as_str),
        Some("/home/me/code")
    );
    cx.update_window(setup.handle.into(), |_, window, cx| {
        state.update(cx, |state, cx| state.go_up(window, cx));
    })
    .unwrap();
    settle(&setup, cx);
    assert_eq!(
        remote.cancelled(),
        1,
        "the request for the folder left was dropped"
    );
    wait(&setup, 500, cx);
    assert!(
        present(&setup, row("code"), cx),
        "back in the parent, listed from cache"
    );
}
