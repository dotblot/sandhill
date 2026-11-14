//! Where a folder picker gets its folders: a trait any file system,
//! remote machine, or API can implement, and the local disk as the
//! default.

use std::{cell::RefCell, collections::HashMap, rc::Rc};

use gpui_kit::{App, AppContext as _, SharedString, Task};

use crate::path_browser::{
    CreateFolderError, Entry, ListError, Page, PageToken, PathStyle, Source, SourcePath,
};
use smol::stream::StreamExt as _;

/// A folder in a listing.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct FolderEntry {
    name: SharedString,
    hidden: bool,
}

impl FolderEntry {
    /// A folder with a name. A name that starts with a dot is hidden.
    pub fn new(name: impl Into<SharedString>) -> Self {
        let name = name.into();
        let hidden = name.starts_with('.');
        Self { name, hidden }
    }

    /// Whether the folder is hidden, which sorts it after the others.
    pub fn with_hidden(mut self, hidden: bool) -> Self {
        self.hidden = hidden;
        self
    }

    /// The folder's name, without a path.
    pub fn name(&self) -> &SharedString {
        &self.name
    }

    /// Whether the folder is hidden.
    pub fn hidden(&self) -> bool {
        self.hidden
    }
}

/// One page of a listing: folders, and where the next page starts.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct FolderPage {
    entries: Vec<FolderEntry>,
    next: Option<PageToken>,
}

impl FolderPage {
    /// The last page of a listing, with `entries` in the order to show.
    pub fn new(entries: impl IntoIterator<Item = FolderEntry>) -> Self {
        Self {
            entries: entries.into_iter().collect(),
            next: None,
        }
    }

    /// Says that more folders follow, from `token`. The picker shows a
    /// "Load more" row and asks for that page when the user reaches it.
    pub fn with_next(mut self, token: PageToken) -> Self {
        self.next = Some(token);
        self
    }

    /// The folders of the page.
    pub fn entries(&self) -> &[FolderEntry] {
        &self.entries
    }

    /// Where the next page starts, when there is one.
    pub fn next(&self) -> Option<&PageToken> {
        self.next.as_ref()
    }
}

/// Where a [`FolderPicker`](super::FolderPicker) lists folders from: the
/// local disk, an SSH session, a bucket, an API. The picker never waits
/// for a source. It calls `list` and awaits the task, and it drops the
/// task to cancel it.
///
/// A source lists one directory a page at a time. It should give entries
/// in the order to show: the picker keeps that order and sorts nothing,
/// so a source that pages must sort across its pages itself. Folders
/// that repeat across pages, by name, show once.
///
/// ```
/// use gpui_cn::{FolderEntry, FolderPage, FolderSource, ListError, PageToken, SourcePath};
/// use gpui_kit::{App, AppContext as _, Task};
///
/// /// A source over a list of names, ten to a page.
/// struct Names(Vec<String>);
///
/// impl FolderSource for Names {
///     fn list(
///         &self,
///         _dir: &SourcePath,
///         page: Option<PageToken>,
///         cx: &mut App,
///     ) -> Task<Result<FolderPage, ListError>> {
///         let start = page.and_then(|t| t.as_str().parse().ok()).unwrap_or(0);
///         let end = (start + 10).min(self.0.len());
///         let names = self.0[start..end].to_vec();
///         let more = (end < self.0.len()).then(|| PageToken::new(end.to_string()));
///         // Any work goes on a task: a request, a query, a disk read.
///         cx.background_spawn(async move {
///             let page = FolderPage::new(names.into_iter().map(FolderEntry::new));
///             Ok(match more {
///                 Some(token) => page.with_next(token),
///                 None => page,
///             })
///         })
///     }
/// }
/// ```
pub trait FolderSource: 'static {
    /// Lists the folders of `dir`, from the start when `page` is `None`,
    /// or else from the page `token` a previous page named. A folder whose
    /// name is empty, `.` or `..`, or has a separator of the source's style
    /// or a control character is dropped by the picker.
    fn list(
        &self,
        dir: &SourcePath,
        page: Option<PageToken>,
        cx: &mut App,
    ) -> Task<Result<FolderPage, ListError>>;

    /// The source's home folder, which a leading `~` in the typed path
    /// stands for and where a picker with no start of its own begins. The
    /// default is `None`: a source with no home does not expand `~`, so a
    /// typed `~` is an ordinary name, and the picker starts at the root.
    /// A remote or virtual source gives its own, made with
    /// [`path_style`](Self::path_style).
    fn home(&self) -> Option<SourcePath> {
        None
    }

    /// How the source writes its paths. The default is the style of the
    /// machine the application runs on. A source for another machine
    /// names its own: a Linux server is [`PathStyle::posix`] whatever
    /// the client is.
    fn path_style(&self) -> PathStyle {
        PathStyle::host()
    }

    /// The value of the variable `name`, for `%NAME%` in a Windows-style
    /// path that is pasted. The default knows none: a remote source never
    /// reads the host's environment.
    fn env(&self, name: &str) -> Option<String> {
        let _ = name;
        None
    }

    /// Whether the source can make folders. The default is `false`, which
    /// hides the picker's "New folder" button.
    fn can_create_folders(&self) -> bool {
        false
    }

    /// Makes the folder `name` inside `parent` and answers with its path.
    /// The picker has checked `name` already: it is not empty, has no
    /// separator, and is not `.` or `..`. Answer
    /// [`CreateFolderError::Exists`] when something has that name. The
    /// picker lists the directory again afterward, so the new folder
    /// shows in the order the source lists. The default answers
    /// [`CreateFolderError::Unsupported`].
    fn create_folder(
        &self,
        parent: &SourcePath,
        name: &str,
        cx: &mut App,
    ) -> Task<Result<SourcePath, CreateFolderError>> {
        let _ = (parent, name, cx);
        Task::ready(Err(CreateFolderError::Unsupported))
    }
}

/// The local file system, in one page. It reads on a background task,
/// counts a symlink to a folder as a folder, and puts the visible folders
/// first and the hidden ones after, each in alphabetical order without
/// regard to case.
#[derive(Clone, Copy, Debug, Default)]
pub struct LocalFolders;

impl FolderSource for LocalFolders {
    /// The home directory of the user running the application.
    fn home(&self) -> Option<SourcePath> {
        std::env::home_dir().map(|home| PathStyle::host().path(home.to_string_lossy()))
    }

    fn env(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }

    fn can_create_folders(&self) -> bool {
        true
    }

    fn create_folder(
        &self,
        parent: &SourcePath,
        name: &str,
        cx: &mut App,
    ) -> Task<Result<SourcePath, CreateFolderError>> {
        let path = parent.join(name);
        let made = path.to_path_buf();
        cx.background_spawn(async move {
            smol::fs::create_dir(&made).await?;
            Ok(path)
        })
    }

    fn list(
        &self,
        dir: &SourcePath,
        _page: Option<PageToken>,
        cx: &mut App,
    ) -> Task<Result<FolderPage, ListError>> {
        let dir = dir.to_path_buf();
        cx.background_spawn(async move { Ok(list_local(dir).await?) })
    }
}

async fn list_local(dir: std::path::PathBuf) -> std::io::Result<FolderPage> {
    let mut read = smol::fs::read_dir(&dir).await?;
    let mut folders = Vec::new();
    while let Some(entry) = read.next().await {
        let Ok(entry) = entry else { continue };
        let is_folder = match entry.file_type().await {
            Ok(kind) if kind.is_dir() => true,
            Ok(kind) if kind.is_symlink() => smol::fs::metadata(entry.path())
                .await
                .is_ok_and(|meta| meta.is_dir()),
            _ => false,
        };
        if is_folder {
            folders.push(FolderEntry::new(
                entry.file_name().to_string_lossy().into_owned(),
            ));
        }
    }
    let style = PathStyle::host();
    folders.sort_by(|a, b| {
        a.hidden()
            .cmp(&b.hidden())
            .then_with(|| style.compare_names(a.name(), b.name()))
    });
    Ok(FolderPage::new(folders))
}

impl Entry for FolderEntry {
    fn name(&self) -> &SharedString {
        &self.name
    }

    fn hidden(&self) -> bool {
        self.hidden
    }

    fn is_folder(&self) -> bool {
        true
    }
}

/// A folder source seen as the browser's source.
pub(super) struct Adapter(pub(super) Rc<dyn FolderSource>);

impl Source<FolderEntry> for Adapter {
    fn list(
        &self,
        dir: &SourcePath,
        page: Option<PageToken>,
        cx: &mut App,
    ) -> Task<Result<Page<FolderEntry>, ListError>> {
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

/// Folders held in memory, for fixtures, demos, and tests. It lists a
/// directory the application named, visible folders first and hidden ones
/// after, fails with [`ListError::NotFound`] for any other, and makes
/// folders: a new one shows in its parent's next listing and is an empty
/// directory itself. Clones share the folders.
#[derive(Clone, Debug, Default)]
pub struct MemoryFolders {
    style: PathStyle,
    home: Option<SourcePath>,
    dirs: Rc<RefCell<HashMap<SourcePath, Vec<FolderEntry>>>>,
}

impl MemoryFolders {
    /// A file system with no directories.
    pub fn new() -> Self {
        Self::default()
    }

    /// Writes the paths in `style`. Set it before the home and the
    /// directories. The default is the host's style.
    pub fn with_style(mut self, style: PathStyle) -> Self {
        self.style = style;
        self
    }

    /// Sets the home folder, which `~` stands for.
    pub fn with_home(mut self, home: impl AsRef<str>) -> Self {
        self.home = Some(self.style.path(home));
        self
    }

    /// Adds the directory `path` with the folders `names`.
    pub fn with_dir<S: Into<SharedString>>(
        self,
        path: impl AsRef<str>,
        names: impl IntoIterator<Item = S>,
    ) -> Self {
        let style = self.style;
        let mut entries: Vec<FolderEntry> = names
            .into_iter()
            .map(|name| {
                let name = name.into();
                let hidden = style.is_hidden_name(&name);
                FolderEntry::new(name).with_hidden(hidden)
            })
            .collect();
        sort_folders(&mut entries, &style);
        self.dirs.borrow_mut().insert(style.path(path), entries);
        self
    }
}

fn sort_folders(entries: &mut [FolderEntry], style: &PathStyle) {
    entries.sort_by(|a, b| {
        a.hidden()
            .cmp(&b.hidden())
            .then_with(|| style.compare_names(a.name(), b.name()))
    });
}

impl FolderSource for MemoryFolders {
    fn home(&self) -> Option<SourcePath> {
        self.home.clone()
    }

    fn path_style(&self) -> PathStyle {
        self.style
    }

    fn list(
        &self,
        dir: &SourcePath,
        _page: Option<PageToken>,
        cx: &mut App,
    ) -> Task<Result<FolderPage, ListError>> {
        let found = self.dirs.borrow().get(dir).cloned();
        cx.background_spawn(async move { found.map(FolderPage::new).ok_or(ListError::NotFound) })
    }

    fn can_create_folders(&self) -> bool {
        true
    }

    fn create_folder(
        &self,
        parent: &SourcePath,
        name: &str,
        _: &mut App,
    ) -> Task<Result<SourcePath, CreateFolderError>> {
        let mut dirs = self.dirs.borrow_mut();
        let style = self.style;
        let Some(entries) = dirs.get_mut(parent) else {
            return Task::ready(Err(CreateFolderError::Other(
                "That directory does not exist".into(),
            )));
        };
        if entries
            .iter()
            .any(|entry| style.fold(entry.name()) == style.fold(name))
        {
            return Task::ready(Err(CreateFolderError::Exists));
        }
        let hidden = style.is_hidden_name(name);
        entries.push(FolderEntry::new(name.to_string()).with_hidden(hidden));
        sort_folders(entries, &style);
        let path = parent.join(name);
        dirs.insert(path.clone(), Vec::new());
        Task::ready(Ok(path))
    }
}
