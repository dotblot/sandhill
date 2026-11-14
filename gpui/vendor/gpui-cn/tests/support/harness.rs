//! One window around either picker, over any source, with the helpers the
//! UI tests of both share: settle, wait, find rows, type, click.

#![allow(dead_code)]

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

use gpui_cn::{
    CreateFolderError, FilePage, FilePicker, FilePickerEvent, FilePickerState, FileSource,
    FolderPage, FolderPicker, FolderPickerEvent, FolderPickerState, FolderSource, ListError,
    PageToken, PathStyle, ReduceMotion, SourcePath, Theme,
};
use gpui_kit::{
    App, AppContext as _, Context, ElementId, Entity, IntoElement, ParentElement as _, Pixels,
    Render, SharedString, Styled as _, Task, TestAppContext, Window, WindowHandle, base::Root, div,
    px, size, test::TestWindowExt as _,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Which {
    Folder,
    File,
}

pub const BOTH: [Which; 2] = [Which::Folder, Which::File];

pub const SELECT_ALL: &str = if cfg!(target_os = "macos") {
    "cmd-a"
} else {
    "ctrl-a"
};

pub const NEW_FOLDER_KEY: &str = if cfg!(target_os = "macos") {
    "cmd-shift-n"
} else {
    "ctrl-shift-n"
};

#[derive(Clone)]
pub enum Picker {
    Folder(Entity<FolderPickerState>),
    File(Entity<FilePickerState>),
}

struct Harness {
    picker: Picker,
    open: Rc<Cell<bool>>,
}

impl Render for Harness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let open = self.open.clone();
        div().size_full().child(match &self.picker {
            Picker::Folder(state) => FolderPicker::new("p", state)
                .open(self.open.get())
                .on_open_change(move |value, _, _| open.set(value))
                .into_any_element(),
            Picker::File(state) => FilePicker::new("p", state)
                .open(self.open.get())
                .on_open_change(move |value, _, _| open.set(value))
                .into_any_element(),
        })
    }
}

/// What a test sets up besides the source.
#[derive(Clone, Default)]
pub struct Options {
    pub initial: Option<String>,
    pub allow_new_folder: bool,
    pub auth: bool,
    pub touch: bool,
    pub multiple: bool,
    pub sign_in: Option<Rc<dyn Fn()>>,
}

pub struct Setup {
    pub which: Which,
    pub handle: WindowHandle<Root>,
    pub picker: Picker,
    pub open: Rc<Cell<bool>>,
    pub folder_events: Rc<RefCell<Vec<FolderPickerEvent>>>,
    pub file_events: Rc<RefCell<Vec<FilePickerEvent>>>,
    pub signed_in: Rc<Cell<usize>>,
}

pub fn start<S: FolderSource + FileSource + Clone>(
    cx: &mut TestAppContext,
    which: Which,
    source: S,
    options: Options,
) -> Setup {
    cx.update(|cx| {
        gpui_kit::init(cx);
        gpui_cn::init(cx);
        Theme::update(cx, |theme| {
            theme.reduce_motion = ReduceMotion::On;
            theme.touch = options.touch;
        });
    });
    let open = Rc::new(Cell::new(true));
    let folder_events = Rc::new(RefCell::new(Vec::new()));
    let file_events = Rc::new(RefCell::new(Vec::new()));
    let signed_in = Rc::new(Cell::new(0));
    let mut picker = None;
    let handle = cx.open_window(size(px(800.), px(800.)), |window, cx| {
        let hook = {
            let signed_in = signed_in.clone();
            let extra = options.sign_in.clone();
            move |_: &mut Window, _: &mut App| {
                signed_in.set(signed_in.get() + 1);
                if let Some(extra) = &extra {
                    extra();
                }
            }
        };
        let built = match which {
            Which::Folder => {
                let state = cx.new(|cx| {
                    let mut state = FolderPickerState::new(window, cx)
                        .with_source(source.clone(), window, cx)
                        .allow_new_folder(options.allow_new_folder);
                    if let Some(initial) = &options.initial {
                        state = state.with_initial(initial, window, cx);
                    }
                    if options.auth {
                        state = state.on_auth_required(hook);
                    }
                    state
                });
                let events = folder_events.clone();
                cx.subscribe(&state, move |_, _, event: &FolderPickerEvent, _| {
                    events.borrow_mut().push(event.clone());
                })
                .detach();
                Picker::Folder(state)
            }
            Which::File => {
                let state = cx.new(|cx| {
                    let mut state = FilePickerState::new(window, cx)
                        .with_source(source.clone(), window, cx)
                        .allow_new_folder(options.allow_new_folder)
                        .with_multiple(options.multiple);
                    if let Some(initial) = &options.initial {
                        state = state.with_initial(initial, window, cx);
                    }
                    if options.auth {
                        state = state.on_auth_required(hook);
                    }
                    state
                });
                let events = file_events.clone();
                cx.subscribe(&state, move |_, _, event: &FilePickerEvent, _| {
                    events.borrow_mut().push(event.clone());
                })
                .detach();
                Picker::File(state)
            }
        };
        picker = Some(built.clone());
        let harness = cx.new(|cx| {
            match &built {
                Picker::Folder(state) => cx.observe(state, |_, _, cx| cx.notify()).detach(),
                Picker::File(state) => cx.observe(state, |_, _, cx| cx.notify()).detach(),
            }
            Harness {
                picker: built,
                open: open.clone(),
            }
        });
        Root::new(harness, window, cx)
    });
    let setup = Setup {
        which,
        handle,
        picker: picker.unwrap(),
        open,
        folder_events,
        file_events,
        signed_in,
    };
    settle(&setup, cx);
    setup
}

pub fn settle(setup: &Setup, cx: &mut TestAppContext) {
    cx.run_until_parked();
    cx.update_window(setup.handle.into(), |_, window, cx| {
        window.activate_window();
        window.render_frame(cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
}

pub fn wait(setup: &Setup, ms: u64, cx: &mut TestAppContext) {
    cx.executor().advance_clock(Duration::from_millis(ms));
    settle(setup, cx);
}

pub fn part(name: &str) -> ElementId {
    ElementId::NamedChild(
        ElementId::Name("p".into()).into(),
        SharedString::from(name.to_string()),
    )
}

/// The row of an entry: the folder picker names it directly, the file
/// picker under `entry`.
pub fn row(name: &str) -> ElementId {
    ElementId::NamedChild(part("entry").into(), name.to_string().into())
}

pub fn present(setup: &Setup, id: ElementId, cx: &mut TestAppContext) -> bool {
    cx.update_window(setup.handle.into(), |_, window, _| {
        window.try_find(id).is_some()
    })
    .unwrap()
}

pub fn click(setup: &Setup, id: ElementId, cx: &mut TestAppContext) {
    cx.update_window(setup.handle.into(), |_, window, cx| window.click(id, cx))
        .unwrap();
    settle(setup, cx);
}

pub fn press(setup: &Setup, key: &str, cx: &mut TestAppContext) {
    cx.update_window(setup.handle.into(), |_, window, cx| window.press(key, cx))
        .unwrap();
    settle(setup, cx);
}

pub fn type_text(setup: &Setup, text: &str, cx: &mut TestAppContext) {
    cx.update_window(setup.handle.into(), |_, window, cx| window.input(text, cx))
        .unwrap();
    settle(setup, cx);
}

pub fn bounds(setup: &Setup, id: ElementId, cx: &mut TestAppContext) -> gpui_kit::Bounds<Pixels> {
    cx.update_window(setup.handle.into(), |_, window, _| window.find(id).bounds())
        .unwrap()
}

/// The height of the bordered list box, which never changes.
pub fn list_height(setup: &Setup, cx: &mut TestAppContext) -> Pixels {
    bounds(setup, part("list"), cx).size.height
}

/// Where the list box ends, which moves if the dialog changes size.
pub fn list_bottom(setup: &Setup, cx: &mut TestAppContext) -> Pixels {
    bounds(setup, part("list"), cx).bottom()
}

pub fn dir(setup: &Setup, cx: &mut TestAppContext) -> String {
    cx.update(|cx| match &setup.picker {
        Picker::Folder(state) => state.read(cx).dir().to_string(),
        Picker::File(state) => state.read(cx).dir().to_string(),
    })
}

pub fn text(setup: &Setup, cx: &mut TestAppContext) -> String {
    cx.update(|cx| match &setup.picker {
        Picker::Folder(state) => state.read(cx).text(cx).to_string(),
        Picker::File(state) => state.read(cx).text(cx).to_string(),
    })
}

pub fn highlighted(setup: &Setup, cx: &mut TestAppContext) -> Option<String> {
    cx.update(|cx| match &setup.picker {
        Picker::Folder(state) => state.read(cx).highlighted().map(|name| name.to_string()),
        Picker::File(state) => state.read(cx).highlighted().map(|name| name.to_string()),
    })
}

pub fn cancel(setup: &Setup, cx: &mut TestAppContext) {
    setup.open.set(false);
    cx.update_window(setup.handle.into(), |_, _, cx| match &setup.picker {
        Picker::Folder(state) => state.update(cx, |state, cx| state.cancel(cx)),
        Picker::File(state) => state.update(cx, |state, cx| state.cancel(cx)),
    })
    .unwrap();
    settle(setup, cx);
}

pub fn retry(setup: &Setup, cx: &mut TestAppContext) {
    cx.update_window(setup.handle.into(), |_, _, cx| match &setup.picker {
        Picker::Folder(state) => state.update(cx, |state, cx| state.retry(cx)),
        Picker::File(state) => state.update(cx, |state, cx| state.retry(cx)),
    })
    .unwrap();
    settle(setup, cx);
}

/// Wraps a folder-only source so the shared harness can open a file picker
/// over the same value, which the folder tests never do.
#[derive(Clone)]
pub struct OnlyFolders<S>(pub S);

impl<S: FolderSource + Clone> FolderSource for OnlyFolders<S> {
    fn list(
        &self,
        dir: &SourcePath,
        page: Option<PageToken>,
        cx: &mut gpui_kit::App,
    ) -> Task<Result<FolderPage, ListError>> {
        self.0.list(dir, page, cx)
    }

    fn home(&self) -> Option<SourcePath> {
        self.0.home()
    }

    fn path_style(&self) -> PathStyle {
        self.0.path_style()
    }

    fn can_create_folders(&self) -> bool {
        self.0.can_create_folders()
    }

    fn create_folder(
        &self,
        parent: &SourcePath,
        name: &str,
        cx: &mut gpui_kit::App,
    ) -> Task<Result<SourcePath, CreateFolderError>> {
        self.0.create_folder(parent, name, cx)
    }
}

impl<S: Clone + 'static> FileSource for OnlyFolders<S> {
    fn list(
        &self,
        _: &SourcePath,
        _: Option<PageToken>,
        cx: &mut gpui_kit::App,
    ) -> Task<Result<FilePage, ListError>> {
        cx.background_spawn(async { Err(ListError::NotFound) })
    }
}

/// Wraps a file-only source so the shared harness can open either picker
/// over the same value; the folder side lists nothing.
#[derive(Clone)]
pub struct OnlyFiles<S>(pub S);

impl<S: FileSource + Clone> FileSource for OnlyFiles<S> {
    fn list(
        &self,
        dir: &SourcePath,
        page: Option<PageToken>,
        cx: &mut gpui_kit::App,
    ) -> Task<Result<FilePage, ListError>> {
        self.0.list(dir, page, cx)
    }

    fn home(&self) -> Option<SourcePath> {
        self.0.home()
    }

    fn path_style(&self) -> PathStyle {
        self.0.path_style()
    }

    fn can_create_folders(&self) -> bool {
        self.0.can_create_folders()
    }

    fn create_folder(
        &self,
        parent: &SourcePath,
        name: &str,
        cx: &mut gpui_kit::App,
    ) -> Task<Result<SourcePath, CreateFolderError>> {
        self.0.create_folder(parent, name, cx)
    }
}

impl<S: Clone + 'static> FolderSource for OnlyFiles<S> {
    fn list(
        &self,
        _: &SourcePath,
        _: Option<PageToken>,
        cx: &mut gpui_kit::App,
    ) -> Task<Result<FolderPage, ListError>> {
        cx.background_spawn(async { Err(ListError::NotFound) })
    }
}
