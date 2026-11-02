//! The files pane's tree: the project's folders, each read from the daemon
//! (`list_dir`) the first time it is opened, as in the Tauri app.

use std::cmp::Ordering;
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

/// How the files are sorted; folders always come first.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) enum SortBy {
    #[default]
    Name,
    Size,
    Modified,
}

impl SortBy {
    pub const ALL: [SortBy; 3] = [SortBy::Name, SortBy::Size, SortBy::Modified];

    pub fn key(self) -> &'static str {
        match self {
            SortBy::Name => "name",
            SortBy::Size => "size",
            SortBy::Modified => "modified",
        }
    }

    pub fn from_key(key: &str) -> SortBy {
        SortBy::ALL
            .into_iter()
            .find(|s| s.key() == key)
            .unwrap_or_default()
    }

    pub fn label(self) -> &'static str {
        match self {
            SortBy::Name => "Name",
            SortBy::Size => "Size",
            SortBy::Modified => "Date modified",
        }
    }

    pub fn order(self) -> &'static str {
        match self {
            SortBy::Name => "A to Z",
            SortBy::Size => "largest first",
            SortBy::Modified => "newest first",
        }
    }
}

/// Names as Finder compares them: ignoring case, and "run2" before "run10".
pub(crate) fn natural(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, _) => return Ordering::Less,
            (_, None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let number = |it: &mut std::iter::Peekable<std::str::Chars>| {
                    let mut digits = String::new();
                    while let Some(c) = it.peek().copied().filter(char::is_ascii_digit) {
                        digits.push(c);
                        it.next();
                    }
                    digits.trim_start_matches('0').to_string()
                };
                let (m, n) = (number(&mut a), number(&mut b));
                let order = m.len().cmp(&n.len()).then_with(|| m.cmp(&n));
                if order != Ordering::Equal {
                    return order;
                }
            }
            (Some(x), Some(y)) => {
                let order = x.to_lowercase().cmp(y.to_lowercase());
                if order != Ordering::Equal {
                    return order;
                }
                a.next();
                b.next();
            }
        }
    }
}

/// A folder's entries in the order chosen, folders first.
pub(crate) fn sorted(entries: &[DirEntry], by: SortBy) -> Vec<&DirEntry> {
    let mut list: Vec<&DirEntry> = entries.iter().collect();
    list.sort_by(|a, b| {
        let dirs = (b.kind == EntryKind::Dir).cmp(&(a.kind == EntryKind::Dir));
        let key = match by {
            SortBy::Name => Ordering::Equal,
            SortBy::Size => b.size.cmp(&a.size),
            SortBy::Modified => b.mtime_ms.cmp(&a.mtime_ms),
        };
        dirs.then(key).then_with(|| natural(&a.name, &b.name))
    });
    list
}

/// "4 min ago", "3 h ago", "yesterday", "12 days ago".
pub(crate) fn modified_ago(mtime_ms: i64, now_ms: i64) -> String {
    let minutes = ((now_ms - mtime_ms).max(0) as f64 / 60_000.).round() as i64;
    match minutes {
        ..1 => "just now".into(),
        1..60 => format!("{minutes} min ago"),
        60..1440 => format!("{} h ago", (minutes as f64 / 60.).round()),
        1440..2880 => "yesterday".into(),
        _ => format!("{} days ago", (minutes as f64 / 1440.).round()),
    }
}

/// A row's tooltip: its name, its size (for files, and folders when sorted
/// by size) and when it changed.
pub(crate) fn row_tip(row: &Row, sized: bool, now_ms: i64) -> String {
    let mut parts = vec![row.name.clone()];
    if sized {
        parts.push(crate::machine::format_bytes(row.size));
    }
    parts.push(format!("modified {}", modified_ago(row.mtime_ms, now_ms)));
    parts.join(" · ")
}

/// Folders by their path in the project ("" is the project itself), which
/// are open, and how files are sorted.
#[derive(Default)]
pub(crate) struct Tree {
    pub folders: HashMap<String, Folder>,
    pub open: HashSet<String>,
    pub sort: SortBy,
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
    pub size: u64,
    pub mtime_ms: i64,
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
    /// An empty tree, sorted as last chosen.
    pub fn new() -> Tree {
        Tree {
            sort: SortBy::from_key(&crate::prefs::tree_sort()),
            ..Tree::default()
        }
    }

    /// The lines to draw: each open folder's entries under it, folders
    /// first, in the order chosen.
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
            size: 0,
            mtime_ms: 0,
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
                for entry in sorted(entries, self.sort)
                    .into_iter()
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
                        size: entry.size,
                        mtime_ms: entry.mtime_ms,
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
    use super::{Folder, Row, SortBy, Tree, natural, sorted};
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

    #[test]
    fn sorts_like_finder() {
        use std::cmp::Ordering;
        assert_eq!(natural("run2", "run10"), Ordering::Less);
        assert_eq!(natural("Beta", "alpha"), Ordering::Greater);
        assert_eq!(natural("a01", "a1"), Ordering::Equal);
        let mut big = entry("big.bam", false);
        big.size = 900;
        big.mtime_ms = 1;
        let mut new = entry("new.csv", false);
        new.size = 10;
        new.mtime_ms = 9;
        let list = [entry("z", true), big, new, entry("a", true)];
        let names = |by| {
            sorted(&list, by)
                .iter()
                .map(|e| e.name.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(names(SortBy::Name), ["a", "z", "big.bam", "new.csv"]);
        assert_eq!(names(SortBy::Size), ["a", "z", "big.bam", "new.csv"]);
        assert_eq!(names(SortBy::Modified), ["a", "z", "new.csv", "big.bam"]);
        assert_eq!(SortBy::from_key("size"), SortBy::Size);
        assert_eq!(SortBy::from_key("odd"), SortBy::Name);
        assert_eq!(super::modified_ago(0, 4 * 60_000), "4 min ago");
        assert_eq!(super::modified_ago(0, 3 * 3_600_000), "3 h ago");
        assert_eq!(super::modified_ago(0, 30 * 3_600_000), "yesterday");
        assert_eq!(super::modified_ago(0, 12 * 86_400_000), "12 days ago");
    }
}
