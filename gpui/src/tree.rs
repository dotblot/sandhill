//! The files pane's tree: the project's folders, each read from the daemon
//! (`list_dir`) the first time it is opened, as in the Tauri app.

use std::collections::{HashMap, HashSet};

use sandhill_proto::{DirEntry, EntryKind};

/// What the tree leaves out, as VS Code does: git's folder, and what macOS,
/// Jupyter and RStudio keep beside the files. Other names that start with a
/// dot, like .env and .gitignore, show.
const HIDDEN: [&str; 4] = [".git", ".DS_Store", ".ipynb_checkpoints", ".Rproj.user"];

pub(crate) enum Folder {
    Loading,
    Ready(Vec<DirEntry>),
    Failed(String),
}

/// Folders by their path in the project ("" is the project itself), and
/// which are open.
#[derive(Default)]
pub(crate) struct Tree {
    pub folders: HashMap<String, Folder>,
    pub open: HashSet<String>,
}

/// One line of the tree.
#[derive(Debug, PartialEq)]
pub(crate) struct Row {
    /// The path in the project, with `/`.
    pub rel: String,
    pub name: String,
    pub depth: usize,
    pub dir: bool,
    pub open: bool,
    /// A note in place of a folder's files: still reading, or why it
    /// couldn't be read.
    pub note: Option<String>,
}

pub(crate) fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
}

impl Tree {
    /// The lines to draw: each open folder's entries under it, folders
    /// first (the daemon's order).
    pub fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        self.add_rows("", 0, &mut rows);
        rows
    }

    fn add_rows(&self, dir: &str, depth: usize, rows: &mut Vec<Row>) {
        let note = |text: String| Row {
            rel: format!("{dir}/\u{0}note"),
            name: String::new(),
            depth,
            dir: false,
            open: false,
            note: Some(text),
        };
        match self.folders.get(dir) {
            None | Some(Folder::Loading) => {
                if !dir.is_empty() {
                    rows.push(note("Reading…".into()));
                }
            }
            Some(Folder::Failed(err)) => rows.push(note(err.clone())),
            Some(Folder::Ready(entries)) => {
                for entry in entries
                    .iter()
                    .filter(|e| !HIDDEN.contains(&e.name.as_str()))
                {
                    let rel = join(dir, &entry.name);
                    let is_dir = entry.kind == EntryKind::Dir;
                    let open = is_dir && self.open.contains(&rel);
                    rows.push(Row {
                        rel: rel.clone(),
                        name: entry.name.clone(),
                        depth,
                        dir: is_dir,
                        open,
                        note: None,
                    });
                    if open {
                        self.add_rows(&rel, depth + 1, rows);
                    }
                }
            }
        }
    }

    /// How many files and folders the project's own folder holds.
    pub fn top_count(&self) -> Option<usize> {
        match self.folders.get("") {
            Some(Folder::Ready(entries)) => Some(
                entries
                    .iter()
                    .filter(|e| !HIDDEN.contains(&e.name.as_str()))
                    .count(),
            ),
            _ => None,
        }
    }

    /// The folders to read again on Refresh: those read and still shown.
    pub fn loaded(&self) -> Vec<String> {
        let mut dirs: Vec<String> = self
            .folders
            .keys()
            .filter(|d| d.is_empty() || self.shown(d))
            .cloned()
            .collect();
        dirs.sort();
        dirs
    }

    /// Whether a folder's entries are on screen: it and every folder above
    /// it are open.
    fn shown(&self, dir: &str) -> bool {
        let mut path = String::new();
        for part in dir.split('/') {
            path = join(&path, part);
            if !self.open.contains(&path) {
                return false;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::{Folder, Row, Tree};
    use sandhill_proto::{DirEntry, EntryKind};

    fn entry(name: &str, dir: bool) -> DirEntry {
        DirEntry {
            name: name.into(),
            kind: if dir { EntryKind::Dir } else { EntryKind::File },
            size: 0,
            mtime_ms: 0,
        }
    }

    fn names(rows: &[Row]) -> Vec<(String, usize)> {
        rows.iter()
            .map(|r| (r.note.clone().unwrap_or(r.rel.clone()), r.depth))
            .collect()
    }

    #[test]
    fn lists_open_folders() {
        let mut tree = Tree::default();
        tree.folders.insert(
            "".into(),
            Folder::Ready(vec![
                entry(".git", true),
                entry("data", true),
                entry("scripts", true),
                entry("qc.qmd", false),
            ]),
        );
        assert_eq!(
            names(&tree.rows()),
            [
                ("data".into(), 0),
                ("scripts".into(), 0),
                ("qc.qmd".into(), 0)
            ]
        );
        tree.open.insert("data".into());
        assert_eq!(names(&tree.rows())[1], ("Reading…".into(), 1));
        tree.folders.insert(
            "data".into(),
            Folder::Ready(vec![entry("raw", true), entry("counts.csv", false)]),
        );
        tree.open.insert("data/raw".into());
        tree.folders
            .insert("data/raw".into(), Folder::Failed("no access".into()));
        assert_eq!(
            names(&tree.rows()),
            [
                ("data".into(), 0),
                ("data/raw".into(), 1),
                ("no access".into(), 2),
                ("data/counts.csv".into(), 1),
                ("scripts".into(), 0),
                ("qc.qmd".into(), 0),
            ]
        );
        assert_eq!(tree.top_count(), Some(3));
        assert_eq!(tree.loaded(), ["", "data", "data/raw"]);
        tree.open.remove("data");
        assert_eq!(tree.loaded(), [""]);
    }
}
