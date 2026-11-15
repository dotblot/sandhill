//! The pickers browse paths in the style of their source, not the host's:
//! a Windows-style source accepts `/` and `\`, a POSIX-style one keeps a
//! backslash as a name character, and typed, pasted and listed paths that
//! name one folder are one folder. Headless, over fake sources that answer
//! at once.

use std::{cell::RefCell, collections::HashMap, rc::Rc};

use gpui_cn::{
    FolderEntry, FolderPage, FolderPicker, FolderPickerEvent, FolderPickerState, FolderSource,
    ListError, PageToken, PathStyle, ReduceMotion, SourcePath, Theme,
};
use gpui_kit::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
    Task, TestAppContext, Window, WindowHandle, base::Root, div, px, size,
    test::TestWindowExt as _,
};

const SELECT_ALL: &str = if cfg!(target_os = "macos") {
    "cmd-a"
} else {
    "ctrl-a"
};

/// A source over folders it holds in memory, in one path style, that
/// remembers every directory it was asked for.
#[derive(Clone)]
struct Remote {
    style: PathStyle,
    home: Option<String>,
    env: HashMap<String, String>,
    dirs: Rc<HashMap<SourcePath, Vec<String>>>,
    log: Rc<RefCell<Vec<String>>>,
}

impl Remote {
    fn new(style: PathStyle, dirs: &[(&str, &[&str])]) -> Self {
        Self {
            style,
            home: None,
            env: HashMap::new(),
            dirs: Rc::new(
                dirs.iter()
                    .map(|(dir, names)| {
                        (
                            style.path(dir),
                            names.iter().map(|name| name.to_string()).collect(),
                        )
                    })
                    .collect(),
            ),
            log: Rc::default(),
        }
    }

    fn with_home(mut self, home: &str) -> Self {
        self.home = Some(home.to_string());
        self
    }

    fn with_env(mut self, name: &str, value: &str) -> Self {
        self.env.insert(name.to_string(), value.to_string());
        self
    }
}

impl FolderSource for Remote {
    fn path_style(&self) -> PathStyle {
        self.style
    }

    fn home(&self) -> Option<SourcePath> {
        self.home.as_deref().map(|home| self.style.path(home))
    }

    fn env(&self, name: &str) -> Option<String> {
        self.env.get(name).cloned()
    }

    fn list(
        &self,
        dir: &SourcePath,
        _: Option<PageToken>,
        cx: &mut App,
    ) -> Task<Result<FolderPage, ListError>> {
        self.log.borrow_mut().push(dir.to_string());
        let found = self.dirs.get(dir).cloned();
        cx.background_spawn(async move {
            found
                .map(|names| FolderPage::new(names.into_iter().map(FolderEntry::new)))
                .ok_or(ListError::NotFound)
        })
    }
}

struct Harness(Entity<FolderPickerState>);

impl Render for Harness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(FolderPicker::new("fp", &self.0).open(true))
    }
}

struct Setup {
    handle: WindowHandle<Root>,
    state: Entity<FolderPickerState>,
    remote: Remote,
    events: Rc<RefCell<Vec<FolderPickerEvent>>>,
}

fn start(cx: &mut TestAppContext, remote: Remote, initial: Option<&str>) -> Setup {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
        Theme::update(cx, |theme| theme.reduce_motion = ReduceMotion::On);
    });
    let events = Rc::new(RefCell::new(Vec::new()));
    let mut state = None;
    let source = remote.clone();
    let handle = cx.open_window(size(px(800.), px(800.)), |window, cx| {
        let entity = cx.new(|cx| {
            let picker = FolderPickerState::new(window, cx).with_source(source, window, cx);
            match initial {
                Some(initial) => picker.with_initial(initial, window, cx),
                None => picker,
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
            Harness(entity)
        });
        Root::new(harness, window, cx)
    });
    let setup = Setup {
        handle,
        state: state.unwrap(),
        remote,
        events,
    };
    settle(&setup, cx);
    setup
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

/// Empties the path field the way a user does.
fn clear(setup: &Setup, cx: &mut TestAppContext) {
    press(setup, SELECT_ALL, cx);
    press(setup, "backspace", cx);
}

fn text(setup: &Setup, cx: &mut TestAppContext) -> String {
    cx.update(|cx| setup.state.read(cx).text(cx).to_string())
}

fn dir(setup: &Setup, cx: &mut TestAppContext) -> String {
    cx.update(|cx| setup.state.read(cx).dir().to_string())
}

fn windows_remote() -> Remote {
    Remote::new(
        PathStyle::windows(),
        &[
            ("C:\\", &["Users", "Windows"]),
            ("C:\\Users", &["me", "you"]),
            ("C:\\Users\\me", &["docs", "My Files"]),
            ("C:\\Users\\me\\docs", &["a"]),
            ("D:\\", &["data"]),
            ("\\\\srv\\share", &["dir"]),
            ("\\\\srv\\share\\dir", &["inner"]),
        ],
    )
    .with_home("C:\\Users\\me")
    .with_env("USERPROFILE", "C:\\Users\\me")
}

fn posix_remote() -> Remote {
    Remote::new(
        PathStyle::posix(),
        &[
            ("/", &["home", "a\\b"]),
            ("/home", &["me"]),
            ("/home/me", &["docs", "Docs"]),
            ("/home/me/x y", &["z"]),
            ("/home/me/docs", &["a"]),
            ("/home/a\\b", &["inner"]),
        ],
    )
    .with_home("/home/me")
}

#[gpui_kit::test]
fn every_spelling_of_a_windows_folder_is_one_directory(cx: &mut TestAppContext) {
    for typed in [
        "C:/Users/me/",
        "C:\\Users\\me\\",
        "C:/Users\\me/",
        "c:\\USERS\\Me\\",
    ] {
        let setup = start(cx, windows_remote(), Some("C:\\"));
        clear(&setup, cx);
        type_text(&setup, typed, cx);
        let shown = dir(&setup, cx);
        assert!(
            !shown.contains('/'),
            "{typed}: written with the style's separator"
        );
        assert_eq!(
            cx.update(|cx| setup.state.read(cx).dir().clone()),
            windows_remote().style.path("C:\\Users\\me"),
            "{typed} is that folder"
        );
        let last = setup.remote.log.borrow().last().cloned().unwrap();
        assert_eq!(
            windows_remote().style.path(&last),
            windows_remote().style.path("C:\\Users\\me"),
            "{typed}: the source was asked for that folder"
        );
    }
}

#[gpui_kit::test]
fn a_listing_is_cached_by_the_canonical_path(cx: &mut TestAppContext) {
    let setup = start(cx, windows_remote(), Some("C:\\Users\\me\\"));
    let asked = setup.remote.log.borrow().len();
    clear(&setup, cx);
    type_text(&setup, "c:/users/me/", cx);
    assert_eq!(
        setup.remote.log.borrow().len(),
        asked + 1,
        "the folder was left, so it is listed again once"
    );
    clear(&setup, cx);
    type_text(&setup, "C:\\USERS\\ME\\", cx);
    clear(&setup, cx);
    type_text(&setup, "C:/Users/me/", cx);
    let asked_before = setup.remote.log.borrow().len();
    clear(&setup, cx);
    type_text(&setup, "C:\\Users\\me\\", cx);
    assert_eq!(
        setup.remote.log.borrow().len(),
        asked_before + 1,
        "a different spelling does not make a second request for one visit"
    );
}

#[gpui_kit::test]
fn a_pasted_windows_path_is_cleaned_once_and_typing_is_left_alone(cx: &mut TestAppContext) {
    let table = [
        ("\"C:\\Users\\me\"", "C:\\Users\\me"),
        ("  C:/Users/me/docs  ", "C:\\Users\\me\\docs"),
        ("C:/Users\\me/docs", "C:\\Users\\me\\docs"),
        ("file:///C:/Users/me%20x", "C:\\Users\\me x"),
        ("C:", "C:\\"),
        ("//srv/share/dir", "\\\\srv\\share\\dir"),
        ("%USERPROFILE%/docs", "C:\\Users\\me\\docs"),
    ];
    for (pasted, shown) in table {
        let setup = start(cx, windows_remote(), Some("C:\\"));
        clear(&setup, cx);
        type_text(&setup, pasted, cx);
        assert_eq!(text(&setup, cx), shown, "{pasted}");
    }
    let setup = start(cx, windows_remote(), Some("C:\\"));
    clear(&setup, cx);
    for key in ["C", ":", "/", "U", "s", "e", "r", "s", "/"] {
        type_text(&setup, key, cx);
    }
    assert_eq!(
        text(&setup, cx),
        "C:/Users/",
        "typing one character at a time never rewrites the text"
    );
    assert_eq!(dir(&setup, cx), "C:\\Users");
}

#[gpui_kit::test]
fn a_posix_source_keeps_a_backslash_and_leaves_pasted_windows_paths_alone(cx: &mut TestAppContext) {
    let setup = start(cx, posix_remote(), Some("/"));
    clear(&setup, cx);
    type_text(&setup, "/home/a\\b/", cx);
    assert_eq!(text(&setup, cx), "/home/a\\b/", "no conversion");
    assert_eq!(dir(&setup, cx), "/home/a\\b");
    clear(&setup, cx);
    type_text(&setup, "C:\\Users\\me", cx);
    assert_eq!(text(&setup, cx), "C:\\Users\\me", "not guessed at");
    cx.update(|cx| {
        assert!(matches!(
            setup.state.read(cx).listing(),
            Some(gpui_cn::Listing::Ready(_))
        ));
    });
    let setup = start(cx, posix_remote(), Some("/"));
    clear(&setup, cx);
    type_text(&setup, "file:///home/me", cx);
    assert_eq!(text(&setup, cx), "/home/me");
    assert_eq!(
        setup.remote.log.borrow().last().map(String::as_str),
        Some("/home")
    );
}

#[gpui_kit::test]
fn the_tilde_is_the_remotes_home_whatever_the_host_is(cx: &mut TestAppContext) {
    let setup = start(cx, windows_remote(), None);
    assert_eq!(
        text(&setup, cx),
        "~\\",
        "the default start is the remote home"
    );
    assert_eq!(dir(&setup, cx), "C:\\Users\\me");
    clear(&setup, cx);
    type_text(&setup, "~/docs/", cx);
    assert_eq!(dir(&setup, cx), "C:\\Users\\me\\docs");
    let setup = start(cx, posix_remote(), None);
    assert_eq!(text(&setup, cx), "~/");
    assert_eq!(dir(&setup, cx), "/home/me");
}

#[gpui_kit::test]
fn up_goes_to_the_parent_and_stops_at_each_kind_of_root(cx: &mut TestAppContext) {
    let up = |setup: &Setup, cx: &mut TestAppContext| {
        cx.update_window(setup.handle.into(), |_, window, cx| {
            setup.state.update(cx, |state, cx| state.go_up(window, cx));
        })
        .unwrap();
        settle(setup, cx);
    };
    let setup = start(cx, windows_remote(), Some("C:\\Users\\me\\docs\\"));
    for expected in ["C:\\Users\\me", "C:\\Users", "C:\\", "C:\\"] {
        up(&setup, cx);
        assert_eq!(dir(&setup, cx), expected);
    }
    let setup = start(cx, windows_remote(), Some("\\\\srv\\share\\dir\\"));
    up(&setup, cx);
    assert_eq!(dir(&setup, cx), "\\\\srv\\share\\");
    up(&setup, cx);
    assert_eq!(dir(&setup, cx), "\\\\srv\\share\\", "a share stays");
    let setup = start(cx, posix_remote(), Some("/home/me/"));
    for expected in ["/home", "/", "/"] {
        up(&setup, cx);
        assert_eq!(dir(&setup, cx), expected);
    }
}

#[gpui_kit::test]
fn a_drive_root_goes_up_to_the_drives_only_when_the_source_lists_them(cx: &mut TestAppContext) {
    let drives = Remote::new(
        PathStyle::windows().with_computer_root(true),
        &[("\\", &["C:", "D:"]), ("C:\\", &["Users"])],
    );
    let setup = start(cx, drives, Some("C:\\"));
    cx.update_window(setup.handle.into(), |_, window, cx| {
        setup.state.update(cx, |state, cx| state.go_up(window, cx));
    })
    .unwrap();
    settle(&setup, cx);
    assert_eq!(dir(&setup, cx), "\\");
    assert_eq!(
        cx.update(|cx| setup.state.read(cx).row_count()),
        2,
        "the drives are listed"
    );
}

#[gpui_kit::test]
fn a_chosen_folder_is_written_in_the_sources_style(cx: &mut TestAppContext) {
    let setup = start(cx, windows_remote(), Some("C:\\"));
    clear(&setup, cx);
    type_text(&setup, "c:/users/me/DOCS", cx);
    cx.update(|cx| setup.state.update(cx, |state, cx| state.choose(cx)));
    settle(&setup, cx);
    let events = setup.events.borrow().clone();
    let [FolderPickerEvent::Chosen(path)] = events.as_slice() else {
        panic!("one choice, got {events:?}");
    };
    assert_eq!(
        path.as_str(),
        "c:\\users\\me\\docs",
        "the listed name, the style's separator"
    );
    assert_eq!(*path, windows_remote().style.path("c:/USERS/me/docs"));
}

#[gpui_kit::test]
fn a_fragment_pasted_into_a_path_only_gets_the_styles_separators(cx: &mut TestAppContext) {
    let setup = start(cx, windows_remote(), Some("C:\\"));
    clear(&setup, cx);
    for key in ["C", ":", "/", "U", "s", "e", "r", "s", "/"] {
        type_text(&setup, key, cx);
    }
    type_text(&setup, "me/docs/", cx);
    assert_eq!(
        text(&setup, cx),
        "C:/Users/me\\docs\\",
        "what was typed stays, the pasted part is written the style's way"
    );
    assert_eq!(dir(&setup, cx), "C:\\Users\\me\\docs");
    let setup = start(cx, windows_remote(), Some("C:\\"));
    clear(&setup, cx);
    type_text(&setup, " \"C:/Users\" ", cx);
    assert_eq!(text(&setup, cx), "C:\\Users", "a whole path is cleaned");
}
