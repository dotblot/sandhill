//! A fake remote machine for the picker tests: folders and files held in
//! memory, in either path style, that answer after a delay, fail on
//! command, ask the user to sign in, and remember every request. Both
//! picker source traits are implemented over it, so one fake serves the
//! folder picker and the file picker.

#![allow(dead_code)]

pub mod harness;

use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, VecDeque},
    rc::Rc,
    time::Duration,
};

use gpui_cn::{
    CreateFolderError, FileEntry, FilePage, FileSource, FolderEntry, FolderPage, FolderSource,
    ListError, PageToken, PathStyle, SourcePath,
};
use gpui_kit::{App, Task};

/// One page of items and where the next starts.
pub type Answer = Result<(Vec<Item>, Option<PageToken>), ListError>;

/// What the fake holds under one directory: names, folder or file.
#[derive(Clone, Debug)]
pub struct Item {
    pub name: String,
    pub folder: bool,
}

pub fn folder(name: &str) -> Item {
    Item {
        name: name.into(),
        folder: true,
    }
}

pub fn file(name: &str) -> Item {
    Item {
        name: name.into(),
        folder: false,
    }
}

/// The state every clone of a fake shares, so a test can steer it after
/// the picker took its copy.
#[derive(Default)]
pub struct Shared {
    pub dirs: RefCell<HashMap<SourcePath, Vec<Item>>>,
    /// Every request, as `dir` or `dir@page`, in order.
    pub log: RefCell<Vec<String>>,
    /// Failures to answer with, one per request, before real answers.
    pub failures: RefCell<VecDeque<ListError>>,
    /// Whether listing needs the user to have signed in.
    pub needs_auth: Cell<bool>,
    pub signed_in: Cell<bool>,
    /// How long every answer takes.
    pub latency: Cell<Duration>,
    /// Entries per page; none lists a directory in one page.
    pub page_size: Cell<Option<usize>>,
    /// How many requests were dropped before they answered.
    pub cancelled: Rc<Cell<usize>>,
    /// Answers sent.
    pub answered: Cell<usize>,
    pub creatable: Cell<bool>,
    /// Folder creations asked for, as `parent|name`.
    pub created: RefCell<Vec<String>>,
    /// Failures to answer folder creation with, one per request.
    pub create_failures: RefCell<VecDeque<CreateFolderError>>,
}

#[derive(Clone)]
pub struct FakeRemote {
    pub style: PathStyle,
    pub home: Option<SourcePath>,
    pub shared: Rc<Shared>,
}

impl FakeRemote {
    pub fn new(style: PathStyle) -> Self {
        let shared = Shared::default();
        shared.creatable.set(true);
        Self {
            style,
            home: None,
            shared: Rc::new(shared),
        }
    }

    pub fn posix() -> Self {
        Self::new(PathStyle::posix())
            .with_home("/home/me")
            .with_dir("/", [folder("home"), folder("srv"), file("motd")])
            .with_dir("/home", [folder("me"), folder("you")])
            .with_dir(
                "/home/me",
                [
                    folder("code"),
                    folder("docs"),
                    folder(".config"),
                    file("notes.txt"),
                    file("todo.md"),
                ],
            )
            .with_dir("/home/me/code", [folder("app"), file("main.rs")])
            .with_dir("/home/me/docs", [file("a.txt")])
            .with_dir("/home/you", [])
            .with_dir("/srv", [folder("www")])
    }

    pub fn windows() -> Self {
        Self::new(PathStyle::windows())
            .with_home("C:\\Users\\me")
            .with_dir(
                "C:\\",
                [folder("Users"), folder("Windows"), file("pagefile.sys")],
            )
            .with_dir("C:\\Users", [folder("me"), folder("Public")])
            .with_dir(
                "C:\\Users\\me",
                [folder("Documents"), folder("Code"), file("notes.txt")],
            )
            .with_dir("C:\\Users\\me\\Code", [file("main.rs")])
            .with_dir("D:\\", [folder("data")])
    }

    pub fn with_home(mut self, home: &str) -> Self {
        self.home = Some(self.style.path(home));
        self
    }

    pub fn with_dir(self, dir: &str, items: impl IntoIterator<Item = Item>) -> Self {
        let mut items: Vec<Item> = items.into_iter().collect();
        let style = self.style;
        items.sort_by(|a, b| {
            b.folder
                .cmp(&a.folder)
                .then_with(|| style.compare_names(&a.name, &b.name))
        });
        self.shared
            .dirs
            .borrow_mut()
            .insert(self.style.path(dir), items);
        self
    }

    pub fn with_latency(self, latency: Duration) -> Self {
        self.shared.latency.set(latency);
        self
    }

    pub fn with_page_size(self, size: usize) -> Self {
        self.shared.page_size.set(Some(size));
        self
    }

    pub fn requiring_sign_in(self) -> Self {
        self.shared.needs_auth.set(true);
        self
    }

    pub fn read_only(self) -> Self {
        self.shared.creatable.set(false);
        self
    }

    pub fn fail_create(&self, error: CreateFolderError) {
        self.shared.create_failures.borrow_mut().push_back(error);
    }

    /// The folders asked for so far, as `parent|name`.
    pub fn created(&self) -> Vec<String> {
        self.shared.created.borrow().clone()
    }

    pub fn fail_next(&self, error: ListError) {
        self.shared.failures.borrow_mut().push_back(error);
    }

    pub fn sign_in(&self) {
        self.shared.signed_in.set(true);
    }

    pub fn requested(&self) -> Vec<String> {
        self.shared.log.borrow().clone()
    }

    pub fn cancelled(&self) -> usize {
        self.shared.cancelled.get()
    }

    /// The names in `dir` now, as the fake holds them.
    pub fn names(&self, dir: &str) -> Vec<String> {
        self.shared
            .dirs
            .borrow()
            .get(&self.style.path(dir))
            .map(|items| items.iter().map(|item| item.name.clone()).collect())
            .unwrap_or_default()
    }

    /// One request: records it, waits out the latency, and answers with a
    /// scripted failure, a sign-in demand, or a page of the directory.
    fn request(&self, dir: &SourcePath, page: Option<PageToken>, cx: &mut App) -> Task<Answer> {
        let shared = self.shared.clone();
        let label = match &page {
            Some(token) => format!("{dir}@{}", token.as_str()),
            None => dir.to_string(),
        };
        shared.log.borrow_mut().push(label);
        let dir = dir.clone();
        let guard = Guard {
            cancelled: shared.cancelled.clone(),
            done: false,
        };
        cx.spawn(async move |cx| {
            let mut guard = guard;
            let latency = shared.latency.get();
            if !latency.is_zero() {
                cx.background_executor().timer(latency).await;
            }
            guard.done = true;
            shared.answered.set(shared.answered.get() + 1);
            if let Some(error) = shared.failures.borrow_mut().pop_front() {
                return Err(error);
            }
            if shared.needs_auth.get() && !shared.signed_in.get() {
                return Err(ListError::AuthRequired(
                    "Sign in to browse this server".into(),
                ));
            }
            let items = shared
                .dirs
                .borrow()
                .get(&dir)
                .cloned()
                .ok_or(ListError::NotFound)?;
            let Some(size) = shared.page_size.get() else {
                return Ok((items, None));
            };
            let start = page.and_then(|t| t.as_str().parse().ok()).unwrap_or(0);
            let end = (start + size).min(items.len());
            let next = (end < items.len()).then(|| PageToken::new(end.to_string()));
            Ok((items[start..end].to_vec(), next))
        })
    }
}

impl FakeRemote {
    fn make(
        &self,
        parent: &SourcePath,
        name: &str,
        cx: &mut App,
    ) -> Task<Result<SourcePath, CreateFolderError>> {
        let shared = self.shared.clone();
        let style = self.style;
        let parent = parent.clone();
        let name = name.to_string();
        shared.created.borrow_mut().push(format!("{parent}|{name}"));
        cx.spawn(async move |cx| {
            let latency = shared.latency.get();
            if !latency.is_zero() {
                cx.background_executor().timer(latency).await;
            }
            if let Some(error) = shared.create_failures.borrow_mut().pop_front() {
                return Err(error);
            }
            let mut dirs = shared.dirs.borrow_mut();
            let items = dirs
                .get_mut(&parent)
                .ok_or_else(|| CreateFolderError::Other("No such directory".into()))?;
            if items
                .iter()
                .any(|item| style.fold(&item.name) == style.fold(&name))
            {
                return Err(CreateFolderError::Exists);
            }
            items.push(folder(&name));
            items.sort_by(|a, b| {
                b.folder
                    .cmp(&a.folder)
                    .then_with(|| style.compare_names(&a.name, &b.name))
            });
            let path = parent.join(&name);
            dirs.insert(path.clone(), Vec::new());
            Ok(path)
        })
    }
}

/// Counts a request that was dropped before it answered.
struct Guard {
    cancelled: Rc<Cell<usize>>,
    done: bool,
}

impl Drop for Guard {
    fn drop(&mut self) {
        if !self.done {
            self.cancelled.set(self.cancelled.get() + 1);
        }
    }
}

impl FolderSource for FakeRemote {
    fn path_style(&self) -> PathStyle {
        self.style
    }

    fn can_create_folders(&self) -> bool {
        self.shared.creatable.get()
    }

    fn create_folder(
        &self,
        parent: &SourcePath,
        name: &str,
        cx: &mut App,
    ) -> Task<Result<SourcePath, CreateFolderError>> {
        self.make(parent, name, cx)
    }

    fn home(&self) -> Option<SourcePath> {
        self.home.clone()
    }

    fn list(
        &self,
        dir: &SourcePath,
        page: Option<PageToken>,
        cx: &mut App,
    ) -> Task<Result<FolderPage, ListError>> {
        let style = self.style;
        let task = self.request(dir, page, cx);
        cx.spawn(async move |_| {
            let (items, next) = task.await?;
            let page = FolderPage::new(items.into_iter().filter(|item| item.folder).map(|item| {
                let hidden = style.is_hidden_name(&item.name);
                FolderEntry::new(item.name).with_hidden(hidden)
            }));
            Ok(match next {
                Some(token) => page.with_next(token),
                None => page,
            })
        })
    }
}

impl FileSource for FakeRemote {
    fn path_style(&self) -> PathStyle {
        self.style
    }

    fn can_create_folders(&self) -> bool {
        self.shared.creatable.get()
    }

    fn create_folder(
        &self,
        parent: &SourcePath,
        name: &str,
        cx: &mut App,
    ) -> Task<Result<SourcePath, CreateFolderError>> {
        self.make(parent, name, cx)
    }

    fn home(&self) -> Option<SourcePath> {
        self.home.clone()
    }

    fn list(
        &self,
        dir: &SourcePath,
        page: Option<PageToken>,
        cx: &mut App,
    ) -> Task<Result<FilePage, ListError>> {
        let style = self.style;
        let task = self.request(dir, page, cx);
        cx.spawn(async move |_| {
            let (items, next) = task.await?;
            let page = FilePage::new(items.into_iter().map(|item| {
                let hidden = style.is_hidden_name(&item.name);
                if item.folder {
                    FileEntry::folder(item.name).with_hidden(hidden)
                } else {
                    FileEntry::file(item.name).with_hidden(hidden)
                }
            }));
            Ok(match next {
                Some(token) => page.with_next(token),
                None => page,
            })
        })
    }
}
