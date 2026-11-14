//! Pickers over a machine that is not this one.
//!
//! [`FolderPicker`](crate::FolderPicker) and [`FilePicker`](crate::FilePicker)
//! never touch the disk themselves. They ask a source, [`FolderSource`](crate::FolderSource) or
//! [`FileSource`](crate::FileSource), and a source can be an SSH session, an SFTP server, an HTTP
//! API, a container, or a bucket. The crate has no network code and no login
//! code: the application brings both.
//!
//! # What a source decides
//!
//! - **The path style.** [`path_style`](crate::FileSource::path_style) names how the
//!   machine writes paths: [`PathStyle::posix`](crate::PathStyle::posix) for a Linux server,
//!   [`PathStyle::windows`](crate::PathStyle::windows) for a Windows one, whatever the host is. The
//!   pickers parse, join, compare, sort and cache with that style, never
//!   with the host's. A Windows style takes `/` and `\` when a path is typed
//!   or pasted, cleans quotes, `file://` URLs and a lone drive, and ignores
//!   case, so `C:/Users/Me` and `c:\users\me` are one folder. A POSIX style
//!   keeps a backslash as a name character. Paths are a [`SourcePath`](crate::SourcePath), which
//!   compares by that style and is only convertible to a host
//!   `PathBuf` for a source that lists the local disk.
//! - **The home.** [`home`](crate::FileSource::home) is what a leading `~` means and
//!   where the picker starts. The host's home is never used for a remote
//!   source.
//! - **The environment.** [`env`](crate::FileSource::env) answers `%NAME%` in a
//!   pasted Windows path, and by default knows nothing.
//! - **Folders.** [`can_create_folders`](crate::FileSource::can_create_folders) and
//!   [`create_folder`](crate::FileSource::create_folder) turn on the "New folder"
//!   button, which the application enables with `allow_new_folder`.
//!
//! # Time and failure
//!
//! Every call returns a task. The picker awaits it off the UI thread, drops
//! it to cancel (when the user moves to another folder, closes the dialog, or
//! switches source), and shows the list's own loading and error states in a
//! box that never changes size. A source answers a failed listing with a typed
//! [`ListError`](crate::ListError):
//!
//! - [`ListError::Disconnected`](crate::ListError::Disconnected) and [`ListError::Other`](crate::ListError::Other) show their message
//!   with a Retry button, which lists the folder again.
//! - [`ListError::AuthRequired`](crate::ListError::AuthRequired) shows its message with a "Sign in" button,
//!   which calls the handler the application gave with
//!   [`on_auth_required`](crate::FilePickerState::on_auth_required). The
//!   application signs the user in however it likes and calls
//!   [`retry`](crate::FilePickerState::retry) when it is done.
//! - [`ListError::NotFound`](crate::ListError::NotFound) and [`ListError::PermissionDenied`](crate::ListError::PermissionDenied) show their
//!   message and nothing else.
//!
//! # A remote source
//!
//! This source is a server held in memory. Its answers would come from a
//! request instead, awaited on a background task, and it pages, asks for a
//! login, and makes folders. Make it with [`PathStyle::posix`](crate::PathStyle::posix) or
//! [`PathStyle::windows`](crate::PathStyle::windows) and the picker behaves the same.
//!
//! ```
//! use std::{
//!     cell::{Cell, RefCell},
//!     collections::HashMap,
//!     rc::Rc,
//! };
//!
//! use gpui_cn::{
//!     CreateFolderError, FileEntry, FilePage, FilePickerState, FileSource, ListError, PageToken,
//!     PathStyle, SourcePath,
//! };
//! use gpui_kit::{App, AppContext as _, Context, Task, Window};
//!
//! /// The folders of a machine, by directory.
//! #[derive(Clone)]
//! struct Server {
//!     style: PathStyle,
//!     home: &'static str,
//!     signed_in: Rc<Cell<bool>>,
//!     dirs: Rc<RefCell<HashMap<SourcePath, Vec<String>>>>,
//! }
//!
//! impl Server {
//!     fn new(style: PathStyle, home: &'static str) -> Self {
//!         let names = |names: &[&str]| names.iter().map(|name| name.to_string()).collect();
//!         let dirs = HashMap::from([(style.path(home), names(&["code", "docs"]))]);
//!         Self {
//!             style,
//!             home,
//!             signed_in: Rc::default(),
//!             dirs: Rc::new(RefCell::new(dirs)),
//!         }
//!     }
//!
//!     fn posix() -> Self {
//!         Self::new(PathStyle::posix(), "/home/me")
//!     }
//!
//!     fn windows() -> Self {
//!         Self::new(PathStyle::windows(), "C:\\Users\\me")
//!     }
//! }
//!
//! impl FileSource for Server {
//!     fn path_style(&self) -> PathStyle {
//!         self.style
//!     }
//!
//!     fn home(&self) -> Option<SourcePath> {
//!         Some(self.style.path(self.home))
//!     }
//!
//!     fn list(
//!         &self,
//!         dir: &SourcePath,
//!         page: Option<PageToken>,
//!         cx: &mut App,
//!     ) -> Task<Result<FilePage, ListError>> {
//!         if !self.signed_in.get() {
//!             return Task::ready(Err(ListError::AuthRequired(
//!                 "Sign in to the server to browse it".into(),
//!             )));
//!         }
//!         let Some(names) = self.dirs.borrow().get(dir).cloned() else {
//!             return Task::ready(Err(ListError::NotFound));
//!         };
//!         // Ten to a page: the token is whatever the source needs to resume.
//!         let start = page.and_then(|t| t.as_str().parse().ok()).unwrap_or(0);
//!         let end = (start + 10).min(names.len());
//!         let next = (end < names.len()).then(|| PageToken::new(end.to_string()));
//!         // A real source awaits its request here, on a background task.
//!         cx.background_spawn(async move {
//!             let page = FilePage::new(names[start..end].iter().cloned().map(FileEntry::folder));
//!             Ok(match next {
//!                 Some(token) => page.with_next(token),
//!                 None => page,
//!             })
//!         })
//!     }
//!
//!     fn can_create_folders(&self) -> bool {
//!         true
//!     }
//!
//!     fn create_folder(
//!         &self,
//!         parent: &SourcePath,
//!         name: &str,
//!         _: &mut App,
//!     ) -> Task<Result<SourcePath, CreateFolderError>> {
//!         let mut dirs = self.dirs.borrow_mut();
//!         let style = self.style;
//!         let Some(names) = dirs.get_mut(parent) else {
//!             return Task::ready(Err(CreateFolderError::PermissionDenied));
//!         };
//!         // Names compare by the style's case rule.
//!         if names.iter().any(|known| style.fold(known) == style.fold(name)) {
//!             return Task::ready(Err(CreateFolderError::Exists));
//!         }
//!         names.push(name.to_string());
//!         let path = parent.join(name);
//!         dirs.insert(path.clone(), Vec::new());
//!         Task::ready(Ok(path))
//!     }
//! }
//!
//! /// A picker over the server, with a login the application owns.
//! fn picker(server: Server, window: &mut Window, cx: &mut Context<FilePickerState>) -> FilePickerState {
//!     let signed_in = server.signed_in.clone();
//!     let this = cx.weak_entity();
//!     FilePickerState::new(window, cx)
//!         .with_source(server, window, cx)
//!         .allow_new_folder(true)
//!         .on_auth_required(move |_, cx| {
//!             // The application's own sign-in goes here, then the list again.
//!             signed_in.set(true);
//!             this.update(cx, |picker, cx| picker.retry(cx)).ok();
//!         })
//! }
//!
//! let _ = (Server::posix(), Server::windows(), picker);
//! ```
//!
//! The same source serves a [`FolderPicker`](crate::FolderPicker) by
//! implementing [`FolderSource`](crate::FolderSource) with the same methods and
//! [`FolderPage`](crate::FolderPage) in place of [`FilePage`](crate::FilePage).
//!
//! The gallery's "Remote (simulated)" switch on the file picker and folder
//! picker pages runs a source like this one, with latency, a folder that
//! drops its connection once, and a folder that needs a login.
