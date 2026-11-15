//! A dialog to choose files on any file system: the path is typed text,
//! and the entries of its directory are listed, filtered fuzzily by what
//! follows the last separator.
//!
//! [`FilePickerState`] holds the path, the listings, the filtered rows,
//! and the selection; [`FilePicker`] draws them in a [`Dialog`]. It shares
//! its browsing with the folder picker. Nothing waits on the UI thread: a
//! [`FileSource`] returns a task for each page of a listing, and the
//! filter runs on a background task that the next keystroke cancels.

mod source;

use std::{collections::HashSet, rc::Rc};

use gpui_kit::{
    App, AppContext as _, Context, ElementId, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, KeyBinding, Modifiers, ParentElement as _, RenderOnce,
    Role, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window,
    actions,
    assets::IconName,
    base::{
        Align, Disableable as _, TestSupportExt as _,
        actions::{Confirm, SelectDown, SelectUp},
        h_flex,
    },
    div,
    prelude::FluentBuilder as _,
};

pub use source::{FileEntry, FileKind, FilePage, FileSource, LocalFiles, MemoryFiles};

use crate::{
    ActiveTheme as _, Button, ButtonSize, Dialog, DropdownMenu, Icon, MenuEntry, MenuEvent,
    MenuItem, MenuState, ScrollArea, Switch, Theme,
    menu::MenuLook,
    path_browser::{
        self, Body, Browser, CreateFolderError, Host, ListError, NewFolder, Page, PageToken,
        PathStyle, Source, SourcePath, Submit, view,
    },
};

/// The key context of the picker, which takes Up, Down, and Enter the
/// path field passes on.
const CONTEXT: &str = "GpuiCnFilePicker";

actions!(
    gpui_cn_file_picker,
    [
        /// Selects every file listed, when the picker takes several.
        SelectAllFiles
    ]
);

/// Cmd+A on macOS, Ctrl+A elsewhere.
const SELECT_ALL_KEY: &str = if cfg!(target_os = "macos") {
    "cmd-a"
} else {
    "ctrl-a"
};

pub(crate) fn init(cx: &mut App) {
    path_browser::bind_keys(CONTEXT, cx);
    // The path field binds this key to select its text. A binding for the
    // field inside the picker is as deep and comes later, so it runs
    // first, and it hands the key on to the field when it does not take it.
    cx.bind_keys([KeyBinding::new(
        SELECT_ALL_KEY,
        SelectAllFiles,
        Some("GpuiCnFilePicker > Input"),
    )]);
}

/// The files of a directory loaded so far, and whether more follow.
pub type FileLoaded = path_browser::Loaded<FileEntry>;

/// What a [`FilePickerState`] knows about one directory.
pub type FileListing = path_browser::Listing<FileEntry>;

/// Which files a picker shows. Folders always show, so the user can go
/// through them to the files.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct FileFilter {
    label: SharedString,
    extensions: Vec<String>,
}

impl FileFilter {
    /// A filter that shows the files with one of `extensions`, named
    /// `label` in the dialog. An extension is written without its dot,
    /// and case does not matter. No extensions shows every file.
    pub fn new(
        label: impl Into<SharedString>,
        extensions: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            label: label.into(),
            extensions: extensions
                .into_iter()
                .map(|extension| {
                    let extension: String = extension.into();
                    extension.trim_start_matches('.').to_lowercase()
                })
                .collect(),
        }
    }

    /// The name the dialog shows.
    pub fn label(&self) -> &SharedString {
        &self.label
    }

    /// The extensions, without a dot, in lower case.
    pub fn extensions(&self) -> &[String] {
        &self.extensions
    }

    /// Whether the filter shows `entry`: every folder, and the files with
    /// one of its extensions.
    pub fn matches(&self, entry: &FileEntry) -> bool {
        if entry.is_folder() || self.extensions.is_empty() {
            return true;
        }
        entry
            .extension()
            .is_some_and(|extension| self.extensions.contains(&extension))
    }
}

/// What a [`FilePickerState`] reports.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum FilePickerEvent {
    /// The files were chosen, in the order the list shows them.
    Confirmed(Vec<SourcePath>),
    /// The picker was cancelled.
    Cancelled,
    /// The user selected or deselected files.
    SelectionChanged,
    /// A folder was made in the directory that is listed, from the "New
    /// folder" row or [`FilePickerState::create_folder`]. The listing is
    /// read again, and the picker puts the highlight on the new folder.
    FolderCreated(SourcePath),
}

/// The path text, the listings, the filtered rows, and the selection of a
/// [`FilePicker`].
///
/// The text is a path. Everything up to and including its last separator
/// is the directory that is listed; what follows is a fuzzy query over
/// that directory's entries. Typing a separator after a query goes inside
/// the folder it names, and a leading `~` stands for the source's home.
/// Folders come first. A double click or Enter goes inside a folder, and
/// the Up button goes to the parent.
///
/// A click selects a file. In a picker that takes
/// [`multiple`](Self::multiple) files, Cmd (Ctrl elsewhere) toggles a
/// file, Shift selects the range from the last file clicked, and Cmd+A
/// selects every file listed while the path has no query. Enter or a double
/// click on a file chooses it, and Cmd+Enter (Ctrl+Enter elsewhere)
/// chooses the selection. The selection belongs to the directory that is
/// listed, and only files that show count: a file the filter or the query
/// hides is not chosen.
pub struct FilePickerState {
    browser: Browser<FileEntry>,
    multiple: bool,
    filters: Vec<FileFilter>,
    active_filter: usize,
    menu: Entity<MenuState>,
    show_hidden: bool,
    selected: HashSet<SharedString>,
    selected_visit: u64,
    /// The selection last reported, so a change is reported once.
    reported: Vec<SourcePath>,
    anchor: Option<SharedString>,
    _subscription: Subscription,
    _menu_events: Subscription,
}

impl Host<FileEntry> for FilePickerState {
    fn browser(&self) -> &Browser<FileEntry> {
        &self.browser
    }

    fn browser_mut(&mut self) -> &mut Browser<FileEntry> {
        &mut self.browser
    }

    fn browser_changed(&mut self, cx: &mut Context<Self>) {
        self.sync_selection(cx);
    }

    fn folder_created(&mut self, path: SourcePath, window: &mut Window, cx: &mut Context<Self>) {
        self.browser.focus_path(window, cx);
        cx.emit(FilePickerEvent::FolderCreated(path));
    }
}

impl EventEmitter<FilePickerEvent> for FilePickerState {}

/// The path field's focus, which holds the keyboard for the picker.
impl Focusable for FilePickerState {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.browser.input.read(cx).focus_handle(cx)
    }
}

impl FilePickerState {
    /// A picker on the local file system, at the user's home folder (the
    /// root when there is none). It takes one file, and hides hidden
    /// entries. Name another start with [`with_initial`](Self::with_initial).
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let browser = Browser::new(Rc::new(Adapter(Rc::new(LocalFiles))), window, cx);
        let subscription = browser.watch(window, cx);
        let start = browser.default_start();
        let menu = cx.new(MenuState::new);
        let menu_events = cx.subscribe(&menu, |this, _, event: &MenuEvent, cx| {
            if let MenuEvent::Activated(key) = event
                && let Ok(index) = key.parse::<usize>()
            {
                this.set_active_filter(index, cx);
            }
        });
        let mut state = Self {
            browser,
            multiple: false,
            filters: Vec::new(),
            active_filter: 0,
            menu,
            show_hidden: false,
            selected: HashSet::new(),
            selected_visit: 0,
            reported: Vec::new(),
            anchor: None,
            _subscription: subscription,
            _menu_events: menu_events,
        };
        state.apply_visible(cx);
        state.browser.set_text(&start, window, cx);
        state
    }

    /// Lists entries from `source` instead of the local file system. The
    /// listings cached so far are dropped and the current directory is
    /// listed again. The picker starts at the new source's home, unless
    /// [`with_initial`](Self::with_initial) named a start, and at the root
    /// when the source has no home. See [`FileSource`] for how to write
    /// one.
    pub fn with_source(
        mut self,
        source: impl FileSource,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        self.set_source(source, window, cx);
        self
    }

    /// Lists from `source` from now on, as [`with_source`](Self::with_source)
    /// does for a picker being built: the listings cached so far are
    /// dropped, the requests in flight are cancelled, and the picker goes
    /// to the new source's home, or keeps the path it was given with
    /// `with_initial`.
    pub fn set_source(
        &mut self,
        source: impl FileSource,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.browser
            .set_source(Rc::new(Adapter(Rc::new(source))), window, cx);
        cx.notify();
    }

    /// Starts in `path`, which is listed at once. A path that starts with
    /// `~` is in the source's home. This wins over the source's home as
    /// the start.
    pub fn with_initial(
        mut self,
        path: impl AsRef<str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        self.browser.start_in(path.as_ref(), window, cx);
        self
    }

    /// Whether the picker takes more than one file. The default is one.
    pub fn with_multiple(mut self, multiple: bool) -> Self {
        self.multiple = multiple;
        self
    }

    /// The file types the user can switch between, such as "All files"
    /// and "Rust files". The first is active. A filter with no extensions
    /// shows every file. With one filter the dialog names it, and with
    /// several it offers a menu. Folders always show.
    pub fn with_filters(mut self, filters: Vec<FileFilter>, cx: &mut Context<Self>) -> Self {
        self.filters = filters;
        self.active_filter = 0;
        self.apply_visible(cx);
        self
    }

    /// Makes the filter at `index` the active one. An index out of range
    /// changes nothing.
    pub fn set_active_filter(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.filters.len() && index != self.active_filter {
            self.active_filter = index;
            self.apply_visible(cx);
        }
    }

    /// Shows or hides the hidden entries. They are hidden by default.
    pub fn set_show_hidden(&mut self, show: bool, cx: &mut Context<Self>) {
        if self.show_hidden != show {
            self.show_hidden = show;
            self.apply_visible(cx);
        }
    }

    fn apply_visible(&mut self, cx: &mut Context<Self>) {
        let filter = self.active_filter().cloned();
        let show_hidden = self.show_hidden;
        self.browser.set_visible(
            move |entry| {
                (show_hidden || !entry.hidden())
                    && filter.as_ref().is_none_or(|filter| filter.matches(entry))
            },
            cx,
        );
        self.sync_selection(cx);
    }

    /// Sets what the "Sign in" button does. A source that needs the user to
    /// log in fails a listing with
    /// [`ListError::AuthRequired`](crate::ListError::AuthRequired), and the
    /// list then shows the source's message with a "Sign in" button that
    /// calls `handler`. The picker holds no login code: the application
    /// signs the user in, then calls [`retry`](Self::retry) to list again.
    /// Without a handler the button is not shown.
    pub fn on_auth_required(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.browser.set_auth_handler(Rc::new(handler));
        self
    }

    /// Lists the directory again: the spinner shows and the source is
    /// asked for the first page. Call it after the user signed in, or let
    /// the "Retry" button call it.
    pub fn retry(&mut self, cx: &mut Context<Self>) {
        self.browser.retry(cx);
        self.sync_selection(cx);
    }

    /// Why the directory could not be listed, when it could not.
    pub fn failure(&self) -> Option<&crate::ListError> {
        self.browser.failure()
    }

    /// Offers a "New folder" button, which opens a row at the top of the
    /// list to name a folder in the directory that is listed. It is off
    /// by default, and shows only when the source
    /// [can make folders](FileSource::can_create_folders).
    pub fn allow_new_folder(mut self, allow: bool) -> Self {
        self.browser.set_allow_new_folder_seed(allow);
        self
    }

    /// Whether the picker is set to offer a "New folder" button.
    pub fn allows_new_folder(&self) -> bool {
        self.browser.allows_new_folder()
    }

    /// Offers or withdraws the "New folder" button.
    pub fn set_allow_new_folder(&mut self, allow: bool, cx: &mut Context<Self>) {
        self.browser.set_allow_new_folder(allow, cx);
    }

    /// Whether the button shows: it is on and the source can make folders.
    pub fn can_create_folder(&self) -> bool {
        self.browser.can_create_folder()
    }

    /// Whether the row that names a new folder is open.
    pub fn is_naming_folder(&self) -> bool {
        self.browser.new_folder.is_some()
    }

    /// The name typed in the row that names a new folder, or `None` while
    /// the row is closed.
    pub fn new_folder_name(&self, cx: &App) -> Option<SharedString> {
        self.browser
            .new_folder
            .as_ref()
            .map(|row| row.input.read(cx).value())
    }

    /// Opens the row that names a new folder, with "Untitled folder"
    /// selected. Nothing is made until the name is confirmed with Enter.
    /// It does nothing while the directory loads, or when the source
    /// cannot make folders.
    pub fn begin_new_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.browser.begin_default_new_folder(window, cx);
    }

    /// Makes the folder `name` in the directory that is listed, the way
    /// the row does: the row opens with `name`, or takes it when it is
    /// already open, the name is checked, the source is asked, and the
    /// result shows in the row. The picker reports
    /// [`FilePickerEvent::FolderCreated`] when the folder exists.
    ///
    /// Fails with [`CreateFolderError::NotReady`] while the directory
    /// loads or a folder is being made, [`CreateFolderError::Unsupported`]
    /// when new folders are off or the source cannot make them, and
    /// [`CreateFolderError::InvalidName`] or
    /// [`CreateFolderError::Exists`] for a name that is refused before the
    /// source is asked.
    pub fn create_folder(
        &mut self,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), CreateFolderError> {
        self.browser.create_folder(name, window, cx)
    }

    /// The path field's state.
    pub fn input(&self) -> &Entity<gpui_kit::base::input::InputState> {
        &self.browser.input
    }

    /// The directory that is listed.
    pub fn dir(&self) -> &SourcePath {
        &self.browser.dir
    }

    /// The query: the text after the last separator.
    pub fn query(&self) -> &str {
        &self.browser.query
    }

    /// The path text, as the field shows it.
    pub fn text(&self, cx: &App) -> SharedString {
        self.browser.input.read(cx).value()
    }

    /// What is known about the directory that is listed.
    pub fn listing(&self) -> Option<&FileListing> {
        self.browser.listing()
    }

    /// How many entries the list shows, not counting the "Load more" row.
    pub fn row_count(&self) -> usize {
        self.browser.row_count()
    }

    /// The name of the entry the keyboard is on.
    pub fn highlighted(&self) -> Option<SharedString> {
        self.browser
            .entry_at(self.browser.highlighted?)
            .map(|(entry, _)| entry.name().clone())
    }

    /// The names the list shows, in order.
    pub fn names(&self) -> Vec<SharedString> {
        (0..self.browser.row_count())
            .filter_map(|row| self.browser.entry_at(row))
            .map(|(entry, _)| entry.name().clone())
            .collect()
    }

    /// Whether the picker takes more than one file.
    pub fn multiple(&self) -> bool {
        self.multiple
    }

    /// The filters the user can switch between.
    pub fn filters(&self) -> &[FileFilter] {
        &self.filters
    }

    /// The index of the active filter.
    pub fn active_filter_index(&self) -> usize {
        self.active_filter
    }

    /// The active filter, when there are any.
    pub fn active_filter(&self) -> Option<&FileFilter> {
        self.filters.get(self.active_filter)
    }

    /// How many files of the listed directory the active filter hides,
    /// not counting hidden files.
    pub fn filtered_out(&self) -> usize {
        let Some(filter) = self.active_filter() else {
            return 0;
        };
        self.browser.loaded().map_or(0, |loaded| {
            loaded
                .entries()
                .iter()
                .filter(|entry| !entry.is_folder() && (self.show_hidden || !entry.hidden()))
                .filter(|entry| !filter.matches(entry))
                .count()
        })
    }

    /// How many hidden files of the listed directory the switch hides.
    pub fn hidden_files(&self) -> usize {
        if self.show_hidden {
            return 0;
        }
        self.browser.loaded().map_or(0, |loaded| {
            loaded
                .entries()
                .iter()
                .filter(|entry| !entry.is_folder() && entry.hidden())
                .count()
        })
    }

    /// How many files the listed directory holds, whatever the filter and
    /// the switch hide.
    pub fn file_count(&self) -> usize {
        self.browser.loaded().map_or(0, |loaded| {
            loaded
                .entries()
                .iter()
                .filter(|entry| !entry.is_folder())
                .count()
        })
    }

    /// Whether hidden entries show.
    pub fn show_hidden(&self) -> bool {
        self.show_hidden
    }

    /// The files selected and showing, in the order of the list.
    pub fn selection(&self) -> Vec<SourcePath> {
        if self.selected_visit != self.browser.visit() {
            return Vec::new();
        }
        (0..self.browser.row_count())
            .filter_map(|row| self.browser.entry_at(row))
            .filter(|(entry, _)| !entry.is_folder() && self.selected.contains(entry.name()))
            .map(|(entry, _)| self.browser.dir.join(entry.name().as_str()))
            .collect()
    }

    fn is_selected(&self, name: &SharedString) -> bool {
        self.selected_visit == self.browser.visit() && self.selected.contains(name)
    }

    /// Starts a selection for this visit to the listed directory, dropping
    /// the one of a visit the picker has left, even when it came back.
    fn retarget(&mut self) {
        if self.selected_visit != self.browser.visit() {
            self.selected.clear();
            self.anchor = None;
            self.selected_visit = self.browser.visit();
        }
    }

    fn set_selection(
        &mut self,
        names: HashSet<SharedString>,
        anchor: Option<SharedString>,
        cx: &mut Context<Self>,
    ) {
        self.sync_selection(cx);
        self.retarget();
        self.selected = names;
        self.anchor = anchor;
        self.sync_selection(cx);
        cx.notify();
    }

    /// Reports a change of the selection that was not made by a click:
    /// leaving the folder, a filter, the hidden switch, or a listing that
    /// no longer holds a selected file.
    fn sync_selection(&mut self, cx: &mut Context<Self>) {
        let now = self.selection();
        if now != self.reported {
            self.reported = now;
            cx.emit(FilePickerEvent::SelectionChanged);
        }
    }

    /// The rows that hold files, as (row, name).
    fn file_rows(&self) -> Vec<(usize, SharedString)> {
        (0..self.browser.row_count())
            .filter_map(|row| {
                let (entry, _) = self.browser.entry_at(row)?;
                (!entry.is_folder()).then(|| (row, entry.name().clone()))
            })
            .collect()
    }

    /// Selects every file that shows. A picker for one file does nothing.
    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        if !self.multiple {
            return;
        }
        let names = self.file_rows().into_iter().map(|(_, name)| name).collect();
        self.set_selection(names, None, cx);
    }

    /// Selects nothing.
    pub fn clear_selection(&mut self, cx: &mut Context<Self>) {
        self.set_selection(HashSet::new(), None, cx);
    }

    /// Chooses the selection, if it is not empty.
    pub fn confirm(&mut self, cx: &mut Context<Self>) {
        let paths = self.selection();
        if !paths.is_empty() {
            self.browser.stop_requests(cx);
            cx.emit(FilePickerEvent::Confirmed(paths));
        }
    }

    /// Cancels the picker and every request to the source that is still
    /// running: their answers never arrive. A directory that was still
    /// loading is listed again when the picker shows.
    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        self.browser.stop_requests(cx);
        cx.emit(FilePickerEvent::Cancelled);
    }

    /// Goes to the parent of the listed directory. The root is its own
    /// parent.
    pub fn go_up(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.browser.go_up(window, cx);
        self.sync_selection(cx);
    }

    /// Goes inside the folder `name` of the listed directory.
    pub fn descend(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.browser.descend(name, window, cx);
        self.sync_selection(cx);
    }

    /// Asks the source for the next page of the listed directory, when it
    /// has one and no request is running. After a failure it asks again.
    pub fn load_more(&mut self, cx: &mut Context<Self>) {
        self.browser.load_more(cx);
    }

    /// Select-all from the keyboard: only for a picker of several files
    /// while the path has no query, so that Cmd+A still selects the text
    /// of a query. Tells whether it took the key.
    fn select_all_key(&mut self, cx: &mut Context<Self>) -> bool {
        let takes =
            self.multiple && self.browser.query.is_empty() && self.browser.new_folder.is_none();
        if takes {
            self.select_all(cx);
        }
        takes
    }

    /// Enter: goes inside the highlighted folder, loads more on the
    /// "Load more" row, or chooses the highlighted file. A file that is
    /// not selected becomes the selection first, so Enter opens what the
    /// keyboard is on. With no highlight it chooses the selection.
    fn enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.browser.highlighted else {
            self.confirm(cx);
            return;
        };
        if row == self.browser.row_count() && self.browser.has_more_row() {
            self.browser.load_more(cx);
            return;
        }
        let Some((entry, _)) = self.browser.entry_at(row) else {
            return;
        };
        let name = entry.name().clone();
        if entry.is_folder() {
            self.browser.descend(&name, window, cx);
            self.sync_selection(cx);
            return;
        }
        if !self.is_selected(&name) {
            self.set_selection(HashSet::from([name.clone()]), Some(name), cx);
        }
        self.confirm(cx);
    }

    /// A click on the row `row`. A folder opens on a double click, or on
    /// a single one when `tap_opens` says the pointer is a finger. A file
    /// is selected: alone, toggled with the secondary modifier, or as a
    /// range with Shift. A double click chooses it.
    fn click_row(
        &mut self,
        row: usize,
        modifiers: Modifiers,
        clicks: usize,
        tap_opens: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.browser.highlighted = Some(row);
        if row == self.browser.row_count() && self.browser.has_more_row() {
            self.browser.load_more(cx);
            return;
        }
        let Some((entry, _)) = self.browser.entry_at(row) else {
            return;
        };
        let name = entry.name().clone();
        if entry.is_folder() {
            if clicks >= 2 || tap_opens {
                self.browser.descend(&name, window, cx);
                self.sync_selection(cx);
            } else {
                cx.notify();
            }
            return;
        }
        self.retarget();
        let mut names = self.selected.clone();
        let mut anchor = Some(name.clone());
        if self.multiple && modifiers.secondary(cx) {
            if !names.remove(&name) {
                names.insert(name.clone());
            }
        } else if self.multiple && modifiers.shift {
            let files = self.file_rows();
            let from = self
                .anchor
                .as_ref()
                .and_then(|anchor| files.iter().position(|(_, file)| file == anchor));
            let to = files.iter().position(|(_, file)| *file == name);
            names = match (from, to) {
                (Some(from), Some(to)) => files[from.min(to)..=from.max(to)]
                    .iter()
                    .map(|(_, file)| file.clone())
                    .collect(),
                _ => HashSet::from([name.clone()]),
            };
            if from.is_some() {
                anchor = self.anchor.clone();
            }
        } else {
            names = HashSet::from([name.clone()]);
        }
        self.set_selection(names, anchor, cx);
        if clicks >= 2 && !modifiers.modified() {
            self.confirm(cx);
        }
    }
}

/// A file source seen as the browser's source.
struct Adapter(Rc<dyn FileSource>);

impl Source<FileEntry> for Adapter {
    fn list(
        &self,
        dir: &SourcePath,
        page: Option<PageToken>,
        cx: &mut App,
    ) -> Task<Result<Page<FileEntry>, ListError>> {
        let task = self.0.list(dir, page, cx);
        cx.spawn(async move |_| {
            let page = task.await?;
            Ok(Page {
                entries: page.entries,
                next: page.next,
            })
        })
    }

    fn home(&self) -> Option<SourcePath> {
        self.0.home()
    }

    fn style(&self) -> PathStyle {
        self.0.path_style()
    }

    fn env(&self, name: &str) -> Option<String> {
        self.0.env(name)
    }

    fn can_create_folders(&self) -> bool {
        self.0.can_create_folders()
    }

    fn create_folder(
        &self,
        parent: &SourcePath,
        name: &str,
        cx: &mut App,
    ) -> Task<Result<SourcePath, CreateFolderError>> {
        self.0.create_folder(parent, name, cx)
    }
}

type CloseHandler = Rc<dyn Fn(&mut Window, &mut App)>;
type OpenChange = Rc<dyn Fn(bool, &mut Window, &mut App)>;

/// A dialog to choose files on a [`FilePickerState`]: an Up button and the
/// path field, a row with the filter and the "Show hidden" switch, a
/// bordered list of the entries, and Cancel and "Open file" in the footer.
///
/// ```
/// use gpui_cn::{FilePicker, FilePickerState};
/// use gpui_kit::{Entity, IntoElement};
///
/// fn picker(state: &Entity<FilePickerState>, open: bool) -> impl IntoElement {
///     FilePicker::new("open-file", state)
///         .open(open)
///         .title("Open a Rust file")
/// }
/// ```
///
/// The application owns the open state: [`open`](Self::open) sets it, and
/// it closes the picker when the state reports
/// [`FilePickerEvent::Confirmed`] or [`FilePickerEvent::Cancelled`]. The
/// id must be stable across frames.
#[derive(IntoElement)]
pub struct FilePicker {
    id: ElementId,
    state: Entity<FilePickerState>,
    open: bool,
    title: Option<SharedString>,
    confirm_label: Option<SharedString>,
    cancel_label: SharedString,
    on_open_change: Option<OpenChange>,
}

impl FilePicker {
    /// A closed picker on `state`, with a stable id.
    pub fn new(id: impl Into<ElementId>, state: &Entity<FilePickerState>) -> Self {
        Self {
            id: id.into(),
            state: state.clone(),
            open: false,
            title: None,
            confirm_label: None,
            cancel_label: "Cancel".into(),
            on_open_change: None,
        }
    }

    /// Shows or hides the picker.
    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    /// The title. The default is "Choose a file", or "Choose files" for a
    /// picker of several.
    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// The label of the button that chooses. The default is "Open file",
    /// or "Open files" for a picker of several.
    pub fn confirm_label(mut self, label: impl Into<SharedString>) -> Self {
        self.confirm_label = Some(label.into());
        self
    }

    /// The label of the button that cancels. The default is "Cancel".
    pub fn cancel_label(mut self, label: impl Into<SharedString>) -> Self {
        self.cancel_label = label.into();
        self
    }

    /// Called when Escape, a press on the backdrop, the close button, or
    /// Cancel asks to close the picker, after the state reports
    /// [`FilePickerEvent::Cancelled`].
    pub fn on_open_change(
        mut self,
        handler: impl Fn(bool, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_open_change = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for FilePicker {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let look = MenuLook::of(theme, window.rem_size());
        let list_height = theme.metrics.folder_list_height;
        let link = theme.link;
        let pointer_cursors = Theme::global(cx).pointer_cursors;
        let state = self.state;
        let child = |name: &'static str| ElementId::NamedChild(self.id.clone().into(), name.into());
        if self.open {
            let state = state.clone();
            window.defer(cx, move |_, cx| {
                state.update(cx, |state, cx| state.browser.ensure_listed(cx));
            });
        }
        let (failure, can_sign_in) = {
            let browser = &state.read(cx).browser;
            (browser.failure().cloned(), browser.auth_handler().is_some())
        };
        let (input, list, body_kind, selected, multiple, filters, menu, show_hidden, hint) = {
            let state = state.read(cx);
            let hint = if state.browser.loaded().is_none() {
                Hint::None
            } else if state.file_count() == 0 {
                Hint::NoFiles
            } else if state.filtered_out() > 0 {
                Hint::Filtered {
                    count: state.filtered_out(),
                    label: state
                        .active_filter()
                        .map(|filter| filter.label.clone())
                        .unwrap_or_default(),
                    can_reset: state.active_filter > 0,
                }
            } else if state.hidden_files() > 0 {
                Hint::Hidden(state.hidden_files())
            } else {
                Hint::None
            };
            (
                state.browser.input.clone(),
                state.browser.list.clone(),
                state.browser.body(),
                state.selection(),
                state.multiple,
                state
                    .filters
                    .iter()
                    .map(|filter| filter.label.clone())
                    .collect::<Vec<_>>(),
                state.menu.clone(),
                state.show_hidden,
                hint,
            )
        };
        let has_selection = !selected.is_empty();
        let focus = input.read(cx).focus_handle(cx);

        let close: CloseHandler = {
            let state = state.clone();
            let handler = self.on_open_change;
            Rc::new(move |window, cx| {
                state.update(cx, |state, cx| state.cancel(cx));
                if let Some(handler) = &handler {
                    handler(false, window, cx);
                }
            })
        };

        let body = match body_kind {
            Body::Loading => view::loading(child("loading"), &look),
            Body::Failed => match failure {
                Some(error) => view::failure::<FileEntry, FilePickerState>(
                    &self.id,
                    &look,
                    &state,
                    &error,
                    can_sign_in,
                ),
                None => div().into_any_element(),
            },
            Body::Empty => view::message(child("message"), &look, view::EMPTY_FILES),
            Body::NoMatch => view::message(child("message"), &look, view::NO_FILE_MATCH),
            Body::List => {
                let rows = Rows {
                    id: self.id.clone(),
                    state: state.clone(),
                    look: look.clone(),
                    pointer_cursors,
                };
                let scrolled = state.clone();
                ScrollArea::list(
                    child("rows"),
                    &list,
                    look.row_height,
                    move |row, window, cx| rows.render(row, window, cx),
                )
                .on_scroll(move |_, cx| {
                    scrolled.update(cx, |state, cx| state.browser_mut().rows_scrolled(cx));
                })
                .size_full()
                .into_any_element()
            }
        };

        let up = {
            let state = state.clone();
            Button::new(child("up"))
                .ghost()
                .icon(Icon::from(IconName::ArrowUp))
                .accessibility_label("Parent folder")
                .on_click(move |_, window, cx| {
                    state.update(cx, |state, cx| state.go_up(window, cx));
                })
        };
        let new_folder = state.read(cx).can_create_folder().then(|| {
            let ready = state.read(cx).browser.new_folder_ready();
            let state = state.clone();
            Button::new(child("new-folder-button"))
                .ghost()
                .icon(Icon::from(IconName::Plus))
                .label("New folder")
                .disabled(!ready)
                .on_click(move |_, window, cx| {
                    state.update(cx, |state, cx| state.begin_new_folder(window, cx));
                })
        });
        let options = h_flex()
            .w_full()
            .items_center()
            .justify_between()
            .gap_2()
            .px(look.row_padding)
            .text_color(look.muted_foreground)
            .child(h_flex().gap_2().items_center().children(new_folder).child(
                if filters.len() > 1 {
                    let active_label = {
                        let state = state.read(cx);
                        state
                            .active_filter()
                            .map(|filter| filter.label.clone())
                            .unwrap_or_default()
                    };
                    let items = state.clone();
                    DropdownMenu::new(child("filter-menu"), &menu)
                        .align(Align::Start)
                        .trigger(
                            Button::new(child("filter-trigger"))
                                .ghost()
                                .size(ButtonSize::Sm)
                                .accessibility_label(active_label.clone())
                                .child(active_label)
                                .trailing_icon(Icon::from(IconName::ChevronDown).size_3()),
                        )
                        .items(move |_, cx| {
                            let state = items.read(cx);
                            state
                                .filters
                                .iter()
                                .enumerate()
                                .map(|(index, filter)| {
                                    MenuEntry::from(
                                        MenuItem::new(index.to_string(), filter.label.clone())
                                            .checked(index == state.active_filter),
                                    )
                                })
                                .collect()
                        })
                        .into_any_element()
                } else {
                    div()
                        .id(child("filter"))
                        .test_support()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .children(filters.first().cloned())
                        .into_any_element()
                },
            ))
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child({
                        let state = state.clone();
                        Switch::new(child("hidden"))
                            .checked(show_hidden)
                            .accessibility_label("Show hidden")
                            .on_change(move |value, _, cx| {
                                let value = *value;
                                state.update(cx, |state, cx| state.set_show_hidden(value, cx));
                            })
                    })
                    .child("Show hidden"),
            )
            .into_any_element();
        let top = {
            let read = state.read(cx);
            read.browser.new_folder.as_ref().map(|row| {
                view::new_folder_row::<FileEntry, FilePickerState>(&self.id, &look, &state, row, cx)
            })
        };
        let (up_key, down_key, enter_key, all_key) =
            (state.clone(), state.clone(), state.clone(), state.clone());
        let content = view::stack()
            .key_context(CONTEXT)
            .on_action(move |_: &SelectAllFiles, _, cx| {
                if !all_key.update(cx, |state, cx| state.select_all_key(cx)) {
                    cx.propagate();
                }
            })
            .on_action(move |_: &SelectUp, _, cx| {
                up_key.update(cx, |state, cx| state.browser.move_highlight(-1, cx));
            })
            .on_action(move |_: &SelectDown, _, cx| {
                down_key.update(cx, |state, cx| state.browser.move_highlight(1, cx));
            })
            .on_action({
                let state = state.clone();
                move |_: &Submit, _, cx| state.update(cx, |state, cx| state.confirm(cx))
            })
            .on_action({
                let state = state.clone();
                move |_: &NewFolder, window, cx| {
                    state.update(cx, |state, cx| state.begin_new_folder(window, cx));
                }
            })
            .on_action(move |_: &Confirm, window, cx| {
                enter_key.update(cx, |state, cx| state.enter(window, cx));
            })
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(up)
                    .child(div().flex_1().child(crate::Input::new(&input))),
            )
            .child(view::slot(&look, Some(options)))
            .child(view::list_box(child("list"), &look, list_height, top, body))
            .child(status_row(
                child("status"),
                &look,
                link,
                pointer_cursors,
                &state,
                multiple,
                &selected,
                hint,
            ));

        let title = self.title.unwrap_or_else(|| {
            if multiple {
                "Choose files"
            } else {
                "Choose a file"
            }
            .into()
        });
        let confirm_label = self.confirm_label.unwrap_or_else(|| {
            match (multiple, selected.len()) {
                (true, 0) => "Open files".to_string(),
                (true, 1) => "Open 1 file".to_string(),
                (true, count) => format!("Open {count} files"),
                (false, _) => "Open file".to_string(),
            }
            .into()
        });
        let footer = h_flex()
            .gap_2()
            .child({
                let close = close.clone();
                Button::new(child("cancel"))
                    .ghost()
                    .label(self.cancel_label)
                    .on_click(move |_, window, cx| close(window, cx))
            })
            .child({
                let state = state.clone();
                Button::new(child("open"))
                    .primary()
                    .label(confirm_label)
                    .disabled(!has_selection)
                    .on_click(move |_, _, cx| state.update(cx, |state, cx| state.confirm(cx)))
            });

        Dialog::new(self.id.clone())
            .open(self.open)
            .title(title)
            .track_focus(&focus)
            .on_open_change(move |_, window, cx| close(window, cx))
            .child(content)
            .footer(footer)
    }
}

/// Why the list may show fewer files than the folder holds.
enum Hint {
    None,
    NoFiles,
    Filtered {
        count: usize,
        label: SharedString,
        can_reset: bool,
    },
    Hidden(usize),
}

fn files_text(count: usize) -> String {
    crate::plural::counted(count, "file")
}

/// The line under the list: what is selected, and why files may be
/// missing, with the action that shows them. Its slot is always there, so
/// the dialog keeps its height.
#[allow(clippy::too_many_arguments)]
fn status_row(
    id: ElementId,
    look: &MenuLook,
    link: gpui_kit::Hsla,
    pointer_cursors: bool,
    state: &Entity<FilePickerState>,
    multiple: bool,
    selected: &[SourcePath],
    hint: Hint,
) -> gpui_kit::AnyElement {
    let selection = match selected {
        [] if multiple => "No files selected".to_string(),
        [] => "No file selected".to_string(),
        [one] => format!("{} selected", one.file_name().unwrap_or_default()),
        many => format!("{} selected", files_text(many.len())),
    };
    let action = |name: &'static str,
                  label: &'static str,
                  run: fn(&mut FilePickerState, &mut Context<FilePickerState>)| {
        let state = state.clone();
        div()
            .id(ElementId::NamedChild(id.clone().into(), name.into()))
            .test_support()
            .role(Role::Button)
            .flex_shrink_0()
            .text_color(link)
            .when(pointer_cursors, |this| this.cursor_pointer())
            .child(label)
            .on_click(move |_, _, cx| state.update(cx, run))
            .into_any_element()
    };
    let note = |text: String| {
        div()
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .child(text)
    };
    let hint = match hint {
        Hint::None => None,
        Hint::NoFiles => Some(h_flex().child(note("No files here.".into()))),
        Hint::Filtered {
            count,
            label,
            can_reset,
        } => Some(
            h_flex()
                .gap_1()
                .child(note(format!(
                    "{} hidden by the {label} filter.",
                    files_text(count)
                )))
                .children(can_reset.then(|| {
                    action("show-all", "Show all", |state, cx| {
                        state.set_active_filter(0, cx)
                    })
                })),
        ),
        Hint::Hidden(count) => Some(
            h_flex()
                .gap_1()
                .child(note(format!("{} hidden.", files_text(count))))
                .child(action("show-hidden", "Show hidden", |state, cx| {
                    state.set_show_hidden(true, cx)
                })),
        ),
    };
    h_flex()
        .id(id.clone())
        .test_support()
        .h(look.row_height)
        .flex_shrink_0()
        .px(look.row_padding)
        .gap_3()
        .items_center()
        .justify_between()
        .overflow_hidden()
        .text_color(look.muted_foreground)
        .child(
            div()
                .id(ElementId::NamedChild(id.into(), "selection".into()))
                .test_support()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .child(selection),
        )
        .children(hint.map(|hint| hint.min_w_0().overflow_hidden()))
        .into_any_element()
}

/// What a row is drawn from, owned by the list's row builder, which
/// outlives the render that made it.
struct Rows {
    id: ElementId,
    state: Entity<FilePickerState>,
    look: MenuLook,
    pointer_cursors: bool,
}

impl Rows {
    fn render(&self, row: usize, _: &mut Window, cx: &mut App) -> gpui_kit::AnyElement {
        let look = &self.look;
        let (name, ranges, highlighted, selected, folder, size, icon) = {
            let state = self.state.read(cx);
            let browser = &state.browser;
            if row == browser.row_count() && browser.has_more_row() {
                return view::more_row::<FileEntry, FilePickerState>(
                    &self.id,
                    look,
                    &self.state,
                    view::MoreRow {
                        state: browser.more_state(),
                        error: browser.more_error().cloned(),
                        can_sign_in: browser.auth_handler().is_some(),
                    },
                    browser.highlighted == Some(row),
                    row,
                    self.pointer_cursors,
                );
            }
            let Some((entry, ranges)) = browser.entry_at(row) else {
                return div().into_any_element();
            };
            (
                entry.name().clone(),
                ranges.to_vec(),
                browser.highlighted == Some(row),
                state.is_selected(entry.name()),
                entry.is_folder(),
                entry.size(),
                icon_of(entry),
            )
        };
        let label = crate::menu::matched_text(name.clone(), &ranges, look);
        let state = self.state.clone();
        let hover = self.state.clone();
        let touch = look.touch;
        view::row_frame(
            ElementId::NamedChild(
                ElementId::NamedChild(self.id.clone().into(), "entry".into()).into(),
                name,
            ),
            look,
            Role::ListBoxOption,
            highlighted,
            selected && !folder,
            self.pointer_cursors,
        )
        .child(
            Icon::from(icon)
                .size_4()
                .flex_shrink_0()
                .text_color(look.muted_foreground),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .child(label),
        )
        .when_some(size, |this, size| {
            this.child(
                div()
                    .flex_shrink_0()
                    .text_color(look.muted_foreground)
                    .child(size_text(size)),
            )
        })
        .when(!folder, |this| {
            this.child(div().w_4().flex_shrink_0().when(selected, |this| {
                this.child(
                    Icon::from(IconName::Check)
                        .size_4()
                        .text_color(look.foreground),
                )
            }))
        })
        .when(!look.touch, |this| {
            this.on_mouse_move(move |_, _, cx| {
                view::highlight_on_hover::<FileEntry, FilePickerState>(&hover, row, cx)
            })
        })
        .on_click(move |event, window, cx| {
            let (modifiers, clicks) = (event.modifiers(), event.click_count());
            state.update(cx, |state, cx| {
                state.click_row(row, modifiers, clicks, touch, window, cx)
            });
        })
        .into_any_element()
    }
}

/// The icon of a row: a folder, a text-like file, or any other file.
fn icon_of(entry: &FileEntry) -> IconName {
    const TEXT: &[&str] = &[
        "txt", "md", "rs", "toml", "json", "yaml", "yml", "js", "ts", "py", "go", "c", "h", "cpp",
        "css", "html", "sh", "lock", "csv", "log",
    ];
    if entry.is_folder() {
        IconName::Folder
    } else if entry
        .extension()
        .is_some_and(|extension| TEXT.contains(&extension.as_str()))
    {
        IconName::FileText
    } else {
        IconName::File
    }
}

/// A size as a row shows it: bytes below a thousand, then kB, MB, and GB,
/// with one decimal below ten.
fn size_text(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["kB", "MB", "GB", "TB"];
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1000.;
    let mut unit = 0;
    while value >= 1000. && unit + 1 < UNITS.len() {
        value /= 1000.;
        unit += 1;
    }
    if value < 10. {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.0} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_in_decimal_units() {
        assert_eq!(size_text(0), "0 B");
        assert_eq!(size_text(999), "999 B");
        assert_eq!(size_text(1500), "1.5 kB");
        assert_eq!(size_text(48_000), "48 kB");
        assert_eq!(size_text(2_400_000), "2.4 MB");
    }

    #[test]
    fn a_filter_keeps_folders_and_matches_extensions_without_case() {
        let filter = FileFilter::new("Rust", [".RS", "toml"]);
        assert!(filter.matches(&FileEntry::folder("src")));
        assert!(filter.matches(&FileEntry::file("Main.rs")));
        assert!(filter.matches(&FileEntry::file("Cargo.toml")));
        assert!(!filter.matches(&FileEntry::file("notes.txt")));
        assert!(!filter.matches(&FileEntry::file("Makefile")));
        assert!(FileFilter::new("All", Vec::<String>::new()).matches(&FileEntry::file("a.b")));
    }
}
