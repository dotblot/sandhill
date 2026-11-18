//! What the files pane does besides opening files, as the Tauri app's
//! `FileTree.tsx` has it: New items (from the + menu, the File menu, ⌘N and
//! ⌥⌘N), naming them in a field in the tree, and the right-click menu with
//! Rename, Duplicate, Reveal in Finder (or Download for a server's file),
//! Copy path and Move to Trash.

use std::collections::HashSet;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::{Sizable, h_flex, v_flex};
use gpui_kit::*;
use sandhill_link::Target;
use sandhill_proto::{ArchiveFormat, Call, Deleted, DirEntry, FileWritten};

use crate::look::{self, Palette, icon, picture};
use crate::panes::files::{
    NewKind, base_name, copy_name, join_path, name_problem, new_notebook, parent_of, template,
    within,
};
use crate::panes::tree::join;
use crate::workbench::Workbench;

/// Keep the rename field and folder guides aligned with the file rows.
pub(crate) fn file_row_indent(depth: usize) -> f32 {
    look::GAP_8 + 16. * depth as f32
}

/// What the name field in the tree is for.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Naming {
    /// A new item in a folder ("" is the project's).
    New { dir: String, kind: NewKind },
    /// Renaming a file or folder.
    Rename { rel: String, dir: bool },
}

/// The right-click menu: on a file or folder, at a point in the window.
pub(crate) struct TreeMenu {
    pub rel: String,
    pub dir: bool,
    pub at: Point<Pixels>,
}

#[derive(Default)]
pub(crate) struct FilesUi {
    /// The file or folder clicked last, which New makes things next to.
    pub selected: Option<(String, bool)>,
    pub naming: Option<Naming>,
    /// Why the name typed can't be used.
    pub error: Option<String>,
    pub menu: Option<TreeMenu>,
    /// The + menu over the files.
    pub new_menu: bool,
    /// The sort menu over the files.
    pub sort_menu: bool,
}

pub(crate) fn new_name_input(
    window: &mut Window,
    cx: &mut Context<Workbench>,
) -> (Entity<InputState>, Subscription) {
    let input = cx.new(|cx| InputState::new(window, cx));
    let subscription = cx.subscribe(&input, |this: &mut Workbench, _, event: &InputEvent, cx| {
        match event {
            InputEvent::PressEnter { .. } => this.commit_name(cx),
            InputEvent::Change if this.files_ui.error.take().is_some() => cx.notify(),
            // Clicking away leaves things as they were.
            InputEvent::Blur if this.files_ui.naming.take().is_some() => {
                this.files_ui.error = None;
                cx.notify();
            }
            _ => {}
        }
    });
    (input, subscription)
}

/// A line of the files pane's list: the tree's own, or the name field.
pub(crate) enum Line {
    Row(crate::panes::tree::Row),
    Field { depth: usize },
}

impl Workbench {
    /// The tree's rows with the name field where it goes: in place of the
    /// row being renamed, or first in the folder a new item goes in.
    pub(crate) fn file_lines(&self) -> Vec<Line> {
        let rows = self.tree.rows();
        let mut lines = Vec::with_capacity(rows.len() + 1);
        match &self.files_ui.naming {
            Some(Naming::New { dir, .. }) if dir.is_empty() => {
                lines.push(Line::Field { depth: 0 });
                lines.extend(rows.into_iter().map(Line::Row));
            }
            Some(Naming::New { dir, .. }) => {
                for row in rows {
                    let here = row.dir && row.rel == *dir;
                    let depth = row.depth + 1;
                    lines.push(Line::Row(row));
                    if here {
                        lines.push(Line::Field { depth });
                    }
                }
            }
            Some(Naming::Rename { rel, .. }) => {
                for row in rows {
                    if row.rel == *rel {
                        lines.push(Line::Field { depth: row.depth });
                    } else {
                        lines.push(Line::Row(row));
                    }
                }
            }
            None => lines.extend(rows.into_iter().map(Line::Row)),
        }
        lines
    }

    /// Where New makes things: the selected folder, the selected file's
    /// folder, or the project's.
    fn target_dir(&self) -> String {
        match &self.files_ui.selected {
            Some((rel, true)) => rel.clone(),
            Some((rel, false)) => match rel.rsplit_once('/') {
                Some((dir, _)) => dir.to_string(),
                None => String::new(),
            },
            None => String::new(),
        }
    }

    /// Opens a folder and every folder above it in the tree.
    pub(crate) fn open_to(&mut self, dir: &str, cx: &mut Context<Self>) {
        let mut at = String::new();
        for part in dir.split('/').filter(|p| !p.is_empty()) {
            at = join(&at, part);
            if self.tree.open.insert(at.clone()) {
                self.read_folder(at.clone(), cx);
            }
        }
    }

    /// Starts naming a new item, in the selected folder unless one is given.
    pub(crate) fn start_new(
        &mut self,
        kind: NewKind,
        dir: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.project.is_none() {
            return;
        }
        let dir = dir.unwrap_or_else(|| self.target_dir());
        self.show_files = true;
        self.open_to(&dir, cx);
        self.files_ui.menu = None;
        self.files_ui.new_menu = false;
        self.files_ui.error = None;
        self.files_ui.naming = Some(Naming::New { dir, kind });
        let start = kind.start_name();
        self.name_input.update(cx, |input, cx| {
            input.set_value(start, window, cx);
            input.focus(window, cx);
            // The name before the extension, ready to type over.
            match start.rfind('.') {
                Some(dot) if dot > 0 => input.set_selected_range(0..dot, cx),
                _ => input.select_all(window, cx),
            }
        });
        cx.notify();
    }

    fn start_rename(
        &mut self,
        rel: String,
        dir: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.files_ui.menu = None;
        self.files_ui.error = None;
        self.files_ui.selected = Some((rel.clone(), dir));
        let name = base_name(&rel).to_string();
        self.files_ui.naming = Some(Naming::Rename { rel, dir });
        self.name_input.update(cx, |input, cx| {
            input.set_value(name.clone(), window, cx);
            input.focus(window, cx);
            match name.rfind('.') {
                Some(dot) if dot > 0 && !dir => input.set_selected_range(0..dot, cx),
                _ => input.select_all(window, cx),
            }
        });
        cx.notify();
    }

    /// Makes or renames what the field names; a problem keeps it open.
    fn commit_name(&mut self, cx: &mut Context<Self>) {
        let (Some(naming), Some(daemon), Some(project)) = (
            self.files_ui.naming.clone(),
            self.daemon(),
            self.project.clone(),
        ) else {
            return;
        };
        let value = self.name_input.read(cx).value().trim().to_string();
        match naming {
            Naming::New { dir, kind } => {
                if let Some(bad) = name_problem(&value, true) {
                    self.files_ui.error = Some(bad.into());
                    cx.notify();
                    return;
                }
                let rel = join(&dir, &value);
                let path = join_path(&project, &rel);
                let call = if kind == NewKind::Folder {
                    Call::MakeDir { path }
                } else {
                    let text = if value.to_ascii_lowercase().ends_with(".ipynb") {
                        let language = self.active_language().unwrap_or_else(|| "python".into());
                        let spec = self
                            .kernels
                            .iter()
                            .find(|k| k.language.eq_ignore_ascii_case(&language))
                            .or_else(|| self.kernels.first());
                        new_notebook(spec)
                    } else {
                        // New paths have no live session. Resolve a remembered
                        // choice before the project default for the fallback
                        // kernel's language, then the first installed kernel.
                        let fallback = self.kernels.first();
                        let project_kernel =
                            fallback.and_then(|spec| self.project_kernel(Some(&spec.language)));
                        let kernel = self
                            .remembered_kernel(&path)
                            .or(project_kernel.as_deref())
                            .or_else(|| fallback.map(|spec| spec.name.as_str()));
                        let language = kernel
                            .and_then(|name| self.kernels.iter().find(|spec| spec.name == name))
                            .map(|spec| spec.language.as_str());
                        template(base_name(&value), language)
                    };
                    Call::WriteFile {
                        path,
                        text,
                        expected_mtime_ms: None,
                        base64: false,
                        create: true,
                    }
                };
                let written = daemon.call::<FileWritten>(call);
                cx.spawn(async move |this, cx| {
                    let written = written.await;
                    this.update(cx, |this, cx| {
                        match written {
                            Ok(_) => {
                                this.files_ui.naming = None;
                                let folder = kind == NewKind::Folder;
                                let parent = match rel.rsplit_once('/') {
                                    Some((dir, _)) => dir.to_string(),
                                    None => String::new(),
                                };
                                this.open_to(if folder { &rel } else { &parent }, cx);
                                this.files_ui.selected = Some((rel.clone(), folder));
                                this.refresh_files(cx);
                                if !folder && crate::editor::notebook::format_of(&rel).is_some() {
                                    this.open_file(rel, cx);
                                }
                            }
                            Err(err) => {
                                this.files_ui.error = Some(problem(&err, &value, &dir));
                            }
                        }
                        cx.notify();
                    })
                    .ok();
                })
                .detach();
            }
            Naming::Rename { rel, dir } => {
                if value.is_empty() || value == base_name(&rel) {
                    self.files_ui.naming = None;
                    cx.notify();
                    return;
                }
                if let Some(bad) = name_problem(&value, false) {
                    self.files_ui.error = Some(bad.into());
                    cx.notify();
                    return;
                }
                let from = join_path(&project, &rel);
                let to = join_path(&parent_of(&from), &value);
                let closed = match self.release(&from, cx) {
                    Ok(closed) => closed,
                    Err(why) => {
                        self.files_ui.error = Some(why);
                        cx.notify();
                        return;
                    }
                };
                let moved = daemon.call::<FileWritten>(Call::MovePath {
                    from: from.clone(),
                    to: to.clone(),
                });
                cx.spawn(async move |this, cx| {
                    let moved = moved.await;
                    this.update(cx, |this, cx| {
                        let reopen: Vec<String> = match &moved {
                            Ok(_) => closed
                                .iter()
                                .map(|p| format!("{to}{}", &p[from.len()..]))
                                .collect(),
                            Err(_) => closed.clone(),
                        };
                        match moved {
                            Ok(_) => {
                                this.files_ui.naming = None;
                                let new_rel = to[project.len()..].trim_start_matches('/');
                                this.files_ui.selected = Some((new_rel.to_string(), dir));
                                // Open folders move with it.
                                let old = rel.clone();
                                let open: HashSet<String> = this
                                    .tree
                                    .open
                                    .drain()
                                    .map(|p| {
                                        if within(&p, &old) {
                                            format!("{new_rel}{}", &p[old.len()..])
                                        } else {
                                            p
                                        }
                                    })
                                    .collect();
                                this.tree.open = open;
                                this.tree.folders.retain(|k, _| !within(k, &old));
                                this.refresh_files(cx);
                                for p in this.tree.open.clone() {
                                    this.read_folder(p, cx);
                                }
                            }
                            Err(err) => {
                                this.files_ui.error = Some(problem(&err, &value, &parent_of(&rel)));
                            }
                        }
                        this.reopen(reopen, cx);
                        cx.notify();
                    })
                    .ok();
                })
                .detach();
            }
        }
    }

    /// Closes the documents open from `path` before it is renamed or
    /// deleted, and returns their paths; a reason when one has unsaved
    /// changes.
    fn release(&mut self, path: &str, cx: &mut Context<Self>) -> Result<Vec<String>, String> {
        let open: Vec<(String, bool)> = self
            .tabs
            .iter()
            .filter(|p| within(p, path))
            .map(|p| (p.clone(), self.tab_dirty(p)))
            .collect();
        if let Some((dirty, _)) = open.iter().find(|(_, dirty)| *dirty) {
            return Err(format!(
                "{} has unsaved changes: save or close it first",
                base_name(dirty)
            ));
        }
        for (p, _) in &open {
            self.close_tab(p, cx);
        }
        Ok(open.into_iter().map(|(p, _)| p).collect())
    }

    fn reopen(&mut self, paths: Vec<String>, cx: &mut Context<Self>) {
        let Some(project) = self.project.clone() else {
            return;
        };
        for path in paths {
            if let Some(rel) = path.strip_prefix(&format!("{project}/")) {
                self.open_file(rel.to_string(), cx);
            }
        }
    }

    fn duplicate(&mut self, rel: String, dir: bool, cx: &mut Context<Self>) {
        let (Some(daemon), Some(project)) = (self.daemon(), self.project.clone()) else {
            return;
        };
        let from = join_path(&project, &rel);
        let parent = parent_of(&from);
        let listed = daemon.call::<Vec<DirEntry>>(Call::ListDir {
            path: parent.clone(),
            sizes: false,
            marks: false,
        });
        cx.spawn(async move |this, cx| {
            let done = async {
                let taken: HashSet<String> = listed.await?.into_iter().map(|e| e.name).collect();
                let to = join_path(&parent, &copy_name(base_name(&from), dir, &taken));
                daemon
                    .call::<FileWritten>(Call::CopyPath {
                        from: from.clone(),
                        to: to.clone(),
                    })
                    .await?;
                anyhow::Ok(to)
            }
            .await;
            this.update(cx, |this, cx| {
                match done {
                    Ok(to) => {
                        let rel = to[project.len()..].trim_start_matches('/').to_string();
                        this.files_ui.selected = Some((rel, dir));
                        this.refresh_files(cx);
                    }
                    Err(err) => {
                        this.problem =
                            Some(format!("Could not duplicate {}: {err:#}", base_name(&from)))
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Moves it to the Trash on this machine; on a server, deletes it once
    /// you say so.
    fn remove(&mut self, rel: String, dir: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(project) = self.project.clone() else {
            return;
        };
        let path = join_path(&project, &rel);
        let remote = self.target != Target::Local;
        if remote {
            let what = if dir {
                "the folder and everything in it"
            } else {
                "it"
            };
            let host = match &self.target {
                Target::Ssh(h) => h.to_string(),
                Target::Local => String::new(),
            };
            let asked = window.prompt(
                PromptLevel::Warning,
                &format!("Delete “{}”?", base_name(&path)),
                Some(&format!(
                    "This deletes {what} on {host}. It can't be put back."
                )),
                &["Delete", "Cancel"],
                cx,
            );
            cx.spawn(async move |this, cx| {
                if asked.await == Ok(0) {
                    this.update(cx, |this, cx| this.delete_path(rel, path, true, cx))
                        .ok();
                }
            })
            .detach();
        } else {
            self.delete_path(rel, path, false, cx);
        }
    }

    fn delete_path(&mut self, rel: String, path: String, remote: bool, cx: &mut Context<Self>) {
        let Some(daemon) = self.daemon() else {
            return;
        };
        let closed = match self.release(&path, cx) {
            Ok(closed) => closed,
            Err(why) => {
                self.problem = Some(why);
                cx.notify();
                return;
            }
        };
        let deleted = daemon.call::<Deleted>(Call::DeletePath {
            path: path.clone(),
            trash: !remote,
        });
        cx.spawn(async move |this, cx| {
            let deleted = deleted.await;
            this.update(cx, |this, cx| {
                match deleted {
                    Ok(_) => {
                        if this
                            .files_ui
                            .selected
                            .as_ref()
                            .is_some_and(|(s, _)| within(s, &rel))
                        {
                            this.files_ui.selected = None;
                        }
                        this.tree.open.retain(|p| !within(p, &rel));
                        this.refresh_files(cx);
                    }
                    Err(err) => {
                        this.reopen(closed, cx);
                        this.problem = Some(format!(
                            "Could not {} {}: {err:#}",
                            if remote {
                                "delete"
                            } else {
                                "move to the Trash"
                            },
                            base_name(&path)
                        ));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Selects a row (New makes things next to it) and, on a right click,
    /// opens its menu.
    pub(crate) fn tree_menu(
        &mut self,
        rel: String,
        dir: bool,
        at: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.files_ui.selected = Some((rel.clone(), dir));
        self.files_ui.menu = Some(TreeMenu { rel, dir, at });
        cx.notify();
    }

    /// The right-click menu, where it was opened.
    pub(crate) fn render_tree_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.files_ui.menu.as_ref()?;
        let p = *Palette::get(cx);
        let project = self.project.clone()?;
        let rel = menu.rel.clone();
        let dir = menu.dir;
        let path = join_path(&project, &rel);
        let local = self.target == Target::Local;
        let mut rows: Vec<AnyElement> = Vec::new();
        if !dir {
            let openable = crate::editor::notebook::can_open(&rel);
            if openable {
                let r = rel.clone();
                rows.push(menu_item(
                    "tm-open",
                    None,
                    "Open in Sandhill",
                    "",
                    &p,
                    cx,
                    move |this, _, cx| this.open_file(r.clone(), cx),
                ));
            }
            let external_path = path.clone();
            rows.push(menu_item(
                "tm-open-external",
                None,
                if local {
                    "Open with Default App"
                } else {
                    "Download to This Mac and Open"
                },
                "",
                &p,
                cx,
                move |this, _, cx| this.open_external(external_path.clone(), cx),
            ));
            if crate::panes::jobs::is_job_script(&rel) {
                let path = path.clone();
                rows.push(menu_item(
                    "tm-job",
                    None,
                    "Run as job",
                    "",
                    &p,
                    cx,
                    move |this, _, cx| {
                        this.start_job(
                            sandhill_proto::JobSpec {
                                path: Some(path.clone()),
                                command: None,
                            },
                            cx,
                        )
                    },
                ));
            }
            if !rows.is_empty() {
                rows.push(separator(&p));
            }
        }
        let folder = if dir {
            rel.clone()
        } else {
            rel.rsplit_once('/')
                .map(|(d, _)| d.to_string())
                .unwrap_or_default()
        };
        let folder_name = if folder.is_empty() {
            base_name(&project).to_string()
        } else {
            base_name(&folder).to_string()
        };
        rows.push(heading(format!("New in {folder_name}"), &p));
        for kind in NewKind::ALL {
            let folder = folder.clone();
            rows.push(menu_item(
                SharedString::from(format!("tm-new-{kind:?}")),
                Some(new_icon(kind)),
                &format!("{}…", kind.label()),
                kind.keys(),
                &p,
                cx,
                move |this, window, cx| this.start_new(kind, Some(folder.clone()), window, cx),
            ));
        }
        rows.push(separator(&p));
        {
            let r = rel.clone();
            rows.push(menu_item(
                "tm-rename",
                Some("edit.rename"),
                "Rename…",
                "↩",
                &p,
                cx,
                move |this, window, cx| this.start_rename(r.clone(), dir, window, cx),
            ));
            let r = rel.clone();
            rows.push(menu_item(
                "tm-dup",
                None,
                "Duplicate",
                "",
                &p,
                cx,
                move |this, _, cx| this.duplicate(r.clone(), dir, cx),
            ));
            let lower = rel.to_ascii_lowercase();
            let tarball = [".tar.gz", ".tgz"].iter().any(|e| lower.ends_with(e));
            let zipped = lower.ends_with(".zip");
            let gzipped = [".gz", ".tgz", ".bgz"].iter().any(|e| lower.ends_with(e));
            if !dir && (zipped || gzipped) {
                let r = rel.clone();
                rows.push(menu_item(
                    "tm-extract",
                    None,
                    if zipped || tarball {
                        "Extract Here"
                    } else {
                        "Decompress Here"
                    },
                    "",
                    &p,
                    cx,
                    move |this, _, cx| this.archive(r.clone(), None, cx),
                ));
            }
            // An archive is offered extraction only: packing it again gains
            // nothing.
            let mut formats = Vec::new();
            if dir || !(zipped || gzipped) {
                formats.push(("tm-zip", "Compress to .zip", ArchiveFormat::Zip));
                formats.push((
                    "tm-gzip",
                    if dir {
                        "Compress to .tar.gz"
                    } else {
                        "Compress to .gz"
                    },
                    ArchiveFormat::Gzip,
                ));
                // bgzip packs one file.
                if !dir {
                    formats.push(("tm-bgzip", "Compress to .gz (bgzip)", ArchiveFormat::Bgzip));
                }
            }
            for (id, label, format) in formats {
                let r = rel.clone();
                rows.push(menu_item(
                    id,
                    None,
                    label,
                    "",
                    &p,
                    cx,
                    move |this, _, cx| this.archive(r.clone(), Some(format), cx),
                ));
            }
        }
        rows.push(separator(&p));
        if local {
            let path = path.clone();
            rows.push(menu_item(
                "tm-reveal",
                Some("folder"),
                if cfg!(target_os = "macos") {
                    "Reveal in Finder"
                } else {
                    "Show in folder"
                },
                "",
                &p,
                cx,
                move |_, _, cx| cx.reveal_path(std::path::Path::new(&path)),
            ));
        }
        if !local && !dir {
            let path = path.clone();
            rows.push(menu_item(
                "tm-download",
                Some("doc.download"),
                "Download to this Mac…",
                "",
                &p,
                cx,
                move |this, _, cx| this.download(path.clone(), cx),
            ));
        }
        {
            let path = path.clone();
            rows.push(menu_item(
                "tm-copy",
                Some("edit.copy"),
                "Copy path",
                "⌥⌘C",
                &p,
                cx,
                move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(path.clone())),
            ));
        }
        rows.push(separator(&p));
        {
            let r = rel.clone();
            rows.push(menu_item(
                "tm-delete",
                Some("delete"),
                if local { "Move to Trash" } else { "Delete…" },
                "⌘⌫",
                &p,
                cx,
                move |this, window, cx| this.remove(r.clone(), dir, window, cx),
            ));
        }
        let body = crate::agents::composer_view::popup(&p)
            .id("tree-menu")
            .occlude()
            .min_w(px(250.))
            .max_h(px(420.))
            .overflow_y_scroll()
            .children(rows)
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.files_ui.menu = None;
                cx.notify();
            }));
        Some(
            deferred(anchored().position(menu.at).snap_to_window().child(body))
                .with_priority(3)
                .into_any_element(),
        )
    }

    /// The + menu over the files: "New in" the selected folder.
    pub(crate) fn render_new_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.files_ui.new_menu {
            return None;
        }
        let p = *Palette::get(cx);
        let project = self.project.clone()?;
        let dir = self.target_dir();
        let name = if dir.is_empty() {
            base_name(&project).to_string()
        } else {
            base_name(&dir).to_string()
        };
        let mut rows = vec![heading(format!("New in {name}"), &p)];
        for kind in NewKind::ALL {
            let dir = dir.clone();
            rows.push(menu_item(
                SharedString::from(format!("nm-{kind:?}")),
                Some(new_icon(kind)),
                &format!("{}…", kind.label()),
                kind.keys(),
                &p,
                cx,
                move |this, window, cx| this.start_new(kind, Some(dir.clone()), window, cx),
            ));
        }
        let body = crate::agents::composer_view::popup(&p)
            .id("new-menu")
            .occlude()
            .min_w(px(250.))
            .children(rows)
            .on_mouse_down_out(cx.listener(|this, event: &MouseDownEvent, _, cx| {
                if this.popup_triggers.contains("files-new", event.position) {
                    return;
                }
                this.files_ui.new_menu = false;
                cx.notify();
            }));
        Some(
            div()
                .absolute()
                .top(px(28.))
                .right(px(look::GAP_8))
                .child(deferred(body).with_priority(3))
                .into_any_element(),
        )
    }

    /// The sort menu over the files: by name, size or date modified, with
    /// a check by the one in use.
    pub(crate) fn render_sort_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.files_ui.sort_menu {
            return None;
        }
        let p = *Palette::get(cx);
        let mut rows = vec![heading("Sort files by".into(), &p)];
        for by in crate::panes::tree::SortBy::ALL {
            rows.push(menu_item(
                SharedString::from(format!("sort-{}", by.key())),
                (by == self.tree.sort).then_some("check"),
                by.label(),
                by.order(),
                &p,
                cx,
                move |this, _, cx| this.set_sort(by, cx),
            ));
        }
        let body = crate::agents::composer_view::popup(&p)
            .id("sort-menu")
            .occlude()
            .min_w(px(240.))
            .children(rows)
            .on_mouse_down_out(cx.listener(|this, event: &MouseDownEvent, _, cx| {
                if this.popup_triggers.contains("files-sort", event.position) {
                    return;
                }
                this.files_ui.sort_menu = false;
                cx.notify();
            }));
        Some(
            div()
                .absolute()
                .top(px(28.))
                .right(px(look::GAP_8))
                .child(deferred(body).with_priority(3))
                .into_any_element(),
        )
    }

    /// The name field's line in the tree, and why the name can't be used.
    pub(crate) fn render_name_field(&self, depth: usize, cx: &mut Context<Self>) -> AnyElement {
        let dir = match &self.files_ui.naming {
            Some(Naming::New { kind, .. }) => *kind == NewKind::Folder,
            Some(Naming::Rename { dir, .. }) => *dir,
            None => false,
        };
        let shown = self.name_input.read(cx).value().to_string();
        let picture_for = if dir {
            look::folder_icon(&shown)
        } else {
            look::file_icon(if shown.is_empty() { "file" } else { &shown })
        };
        h_flex()
            .id("name-field")
            .w_full()
            .h(px(look::ROW))
            .pl(px(file_row_indent(depth) + look::ICON + look::GAP_4))
            .pr(px(look::GAP_8))
            .gap(px(look::GAP_4))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    this.files_ui.naming = None;
                    this.files_ui.error = None;
                    cx.notify();
                }
            }))
            .child(picture(&picture_for).size(px(look::ICON)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(gpui_kit::component::input::Input::new(&self.name_input).xsmall()),
            )
            .into_any_element()
    }

    /// Why the typed name can't be used, under the files pane's heading.
    pub(crate) fn render_name_error(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let error = self.files_ui.error.clone()?;
        let p = *Palette::get(cx);
        Some(
            v_flex()
                .mx(px(look::PANE_PAD_X))
                .mb(px(look::GAP_4))
                .px(px(look::GAP_8))
                .py(px(look::GAP_4))
                .rounded(px(look::CONTROL_RADIUS))
                .bg(p.err.opacity(0.12))
                .text_color(p.err)
                .text_size(px(look::SMALL))
                .child(error)
                .into_any_element(),
        )
    }
}

fn problem(err: &anyhow::Error, name: &str, dir: &str) -> String {
    let text = format!("{err:#}");
    if text.contains("exists:") || text.contains("already exists") {
        let place = if dir.is_empty() {
            "this folder".to_string()
        } else {
            base_name(dir).to_string()
        };
        format!("“{name}” already exists in {place}")
    } else {
        text
    }
}

fn new_icon(kind: NewKind) -> &'static str {
    match kind {
        NewKind::Folder => "folder.new",
        _ => "file.new",
    }
}

fn heading(text: String, p: &Palette) -> AnyElement {
    div()
        .px(px(look::GAP_8))
        .py(px(look::GAP_4))
        .text_size(px(look::SMALL))
        .text_color(p.faint)
        .child(text)
        .into_any_element()
}

pub(crate) fn separator(p: &Palette) -> AnyElement {
    div()
        .my(px(look::GAP_4))
        .h(px(0.5))
        .bg(p.glass_edge)
        .into_any_element()
}

/// One item of a pop-up menu: an icon (or the space for one), its label and
/// its keys at the right. Choosing it closes the menus.
pub(crate) fn menu_item(
    id: impl Into<ElementId>,
    icon_name: Option<&'static str>,
    label: &str,
    keys: &'static str,
    p: &Palette,
    cx: &mut Context<Workbench>,
    run: impl Fn(&mut Workbench, &mut Window, &mut Context<Workbench>) + 'static,
) -> AnyElement {
    let hover = p.ink(look::ROW_HOVER);
    h_flex()
        .id(id)
        .h(px(look::ROW))
        .px(px(look::GAP_8))
        .gap(px(look::GAP_4))
        .rounded(px(look::CONTROL_RADIUS))
        .cursor_pointer()
        .hover(move |d| d.bg(hover))
        .child(
            div()
                .w(px(look::ICON))
                .flex_none()
                .text_color(p.muted)
                .children(icon_name.map(|n| icon(n).size(px(look::ICON)))),
        )
        .child(div().flex_1().whitespace_nowrap().child(label.to_string()))
        .child(div().flex_none().text_color(p.faint).child(keys))
        .on_click(cx.listener(move |this, _, window, cx| {
            this.files_ui.menu = None;
            this.files_ui.new_menu = false;
            this.files_ui.sort_menu = false;
            this.main_menu = false;
            run(this, window, cx);
            cx.notify();
        }))
        .into_any_element()
}
