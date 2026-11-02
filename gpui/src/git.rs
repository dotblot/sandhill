//! What the Git pane shows, worked out from what the daemon reports, as the
//! Tauri app's `model/git.ts` does: file marks, line counts, diff lines,
//! commit messages, branch names, the commit graph and avatars.

use sandhill_proto::{GitCommit, GitFile, GitStatus};

/// GitHub warns about files over 50 MB and refuses them over 100 MB.
pub(crate) const BIG_FILE: u64 = 50 * 1024 * 1024;

/// Files with a staged change.
pub(crate) fn staged_files(status: &GitStatus) -> Vec<&GitFile> {
    status
        .files
        .iter()
        .filter(|f| f.staged.is_some() && !f.conflict)
        .collect()
}

/// Files with a change not staged (new and conflicted ones too).
pub(crate) fn unstaged_files(status: &GitStatus) -> Vec<&GitFile> {
    status
        .files
        .iter()
        .filter(|f| f.unstaged.is_some())
        .collect()
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum MarkKind {
    Added,
    Deleted,
    Renamed,
    Modified,
    Conflict,
}

/// The letter a file shows and what it means: a new file is A, a conflict !.
pub(crate) fn file_mark(file: &GitFile, staged: bool) -> (&'static str, MarkKind, String) {
    if !staged && file.conflict {
        return (
            "!",
            MarkKind::Conflict,
            "Conflict: changed on both sides".into(),
        );
    }
    let letter = if staged {
        file.staged.as_deref()
    } else {
        file.unstaged.as_deref()
    }
    .unwrap_or("M");
    let from = |what: &str| match &file.from {
        Some(from) => format!("{what} from {from}"),
        None => what.to_string(),
    };
    match letter {
        "?" => ("A", MarkKind::Added, "New, not yet in Git".into()),
        "A" => ("A", MarkKind::Added, "Added".into()),
        "D" => ("D", MarkKind::Deleted, "Deleted".into()),
        "R" => ("R", MarkKind::Renamed, from("Renamed")),
        "C" => ("C", MarkKind::Renamed, from("Copied")),
        "T" => ("T", MarkKind::Modified, "Type changed".into()),
        _ => ("M", MarkKind::Modified, "Modified".into()),
    }
}

/// "+12 −3", or "binary".
pub(crate) fn line_counts(lines: Option<&sandhill_proto::GitLines>) -> String {
    let Some(lines) = lines else {
        return String::new();
    };
    if lines.binary {
        return "binary".into();
    }
    let mut parts = Vec::new();
    if lines.added > 0 {
        parts.push(format!("+{}", lines.added));
    }
    if lines.removed > 0 {
        parts.push(format!("−{}", lines.removed));
    }
    parts.join(" ")
}

pub(crate) fn file_size(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let units = ["KB", "MB", "GB", "TB"];
    let mut value = bytes as f64 / 1024.;
    let mut unit = 0;
    while value >= 1024. && unit < units.len() - 1 {
        value /= 1024.;
        unit += 1;
    }
    if value < 10. {
        format!("{value:.1} {}", units[unit])
    } else {
        format!("{} {}", value.round(), units[unit])
    }
}

/// What GitHub makes of a file this big.
pub(crate) fn too_big(bytes: u64) -> &'static str {
    if bytes > 100 * 1024 * 1024 {
        "GitHub refuses files over 100 MB, so pushing a commit with it would fail."
    } else {
        "GitHub warns about files over 50 MB."
    }
}

/// A file's name and its folder, for a row.
pub(crate) fn split_path(path: &str) -> (String, String) {
    let dir_mark = path.ends_with('/');
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(at) => (
            format!("{}{}", &trimmed[at + 1..], if dir_mark { "/" } else { "" }),
            trimmed[..at].to_string(),
        ),
        None => (path.to_string(), String::new()),
    }
}

/// The message for git from the summary and description boxes.
pub(crate) fn commit_message(summary: &str, body: &str) -> String {
    let head = summary.trim();
    let rest = body.trim();
    if rest.is_empty() {
        head.to_string()
    } else {
        format!("{head}\n\n{rest}")
    }
}

/// A message split back into the two boxes, to amend it.
pub(crate) fn split_message(message: &str) -> (String, String) {
    let mut lines = message.split('\n');
    let summary = lines.next().unwrap_or("").trim().to_string();
    let body = lines.collect::<Vec<_>>().join("\n").trim().to_string();
    (summary, body)
}

/// A branch name git would take: no spaces, nothing it reserves.
pub(crate) fn branch_name(text: &str) -> Option<String> {
    let name = text.trim();
    if name.is_empty()
        || name == "@"
        || name.starts_with('-')
        || name.starts_with('/')
        || name.ends_with('/')
        || name.ends_with(".lock")
        || name.ends_with('.')
        || name.contains("..")
        || name.contains("@{")
        || name.contains("//")
        || name
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || "~^:?*[\\".contains(c))
    {
        return None;
    }
    Some(name.to_string())
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum DiffKind {
    Hunk,
    Add,
    Del,
    Context,
    Note,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DiffLine {
    pub kind: DiffKind,
    pub text: String,
    /// Its place among the lines of the diff's text, which line staging names.
    pub at: usize,
    pub old: Option<u32>,
    pub new: Option<u32>,
}

fn hunk_starts(line: &str) -> (u32, u32) {
    let mut old = 0;
    let mut new = 0;
    for word in line.split_whitespace() {
        let number = |w: &str| {
            w.split(',')
                .next()
                .and_then(|n| n.parse::<u32>().ok())
                .unwrap_or(0)
        };
        if let Some(rest) = word.strip_prefix('-') {
            if old == 0 {
                old = number(rest);
            }
        } else if let Some(rest) = word.strip_prefix('+')
            && new == 0
        {
            new = number(rest);
        }
    }
    (old, new)
}

/// Reads a unified diff (or a merge conflict's combined one) into lines
/// with their numbers.
pub(crate) fn diff_lines(text: &str) -> Vec<DiffLine> {
    let mut lines: Vec<DiffLine> = Vec::new();
    let (mut old, mut now, mut width) = (0u32, 0u32, 1usize);
    for (at, line) in text.split('\n').enumerate() {
        if line.starts_with("@@") {
            width = if line.starts_with("@@@") { 2 } else { 1 };
            (old, now) = hunk_starts(line);
            lines.push(DiffLine {
                kind: DiffKind::Hunk,
                text: line.to_string(),
                at,
                old: None,
                new: None,
            });
            continue;
        }
        if let Some(rest) = line.strip_prefix('\\') {
            lines.push(DiffLine {
                kind: DiffKind::Note,
                text: rest.trim_start().to_string(),
                at,
                old: None,
                new: None,
            });
            continue;
        }
        if lines.is_empty() {
            continue;
        }
        let split = line
            .char_indices()
            .nth(width)
            .map_or(line.len(), |(i, _)| i);
        let (mark, body) = line.split_at(split);
        let body = body.to_string();
        if mark.contains('+') {
            lines.push(DiffLine {
                kind: DiffKind::Add,
                text: body,
                at,
                old: None,
                new: Some(now),
            });
            now += 1;
        } else if mark.contains('-') {
            lines.push(DiffLine {
                kind: DiffKind::Del,
                text: body,
                at,
                old: Some(old),
                new: None,
            });
            old += 1;
        } else if !line.is_empty() || lines.last().is_some_and(|l| l.kind != DiffKind::Hunk) {
            lines.push(DiffLine {
                kind: DiffKind::Context,
                text: body,
                at,
                old: Some(old),
                new: Some(now),
            });
            old += 1;
            now += 1;
        }
    }
    // The text ends with a newline, which isn't a line.
    if lines
        .last()
        .is_some_and(|l| l.kind == DiffKind::Context && l.text.is_empty())
        && !text.ends_with("\n\n")
    {
        lines.pop();
    }
    lines
}

/// A hunk's header in words: "Lines 33–38" of the new file, and the code
/// it's in.
pub(crate) fn hunk_label(header: &str) -> (String, String) {
    let trimmed = header.trim_start_matches('@').trim_start();
    let Some(close) = trimmed.find(" @@") else {
        return (header.to_string(), String::new());
    };
    let ranges = &trimmed[..close];
    let context = trimmed[close..]
        .trim_start_matches(' ')
        .trim_start_matches('@')
        .trim_start()
        .to_string();
    let Some(plus) = ranges.split_whitespace().find_map(|w| w.strip_prefix('+')) else {
        return (header.to_string(), String::new());
    };
    let mut parts = plus.split(',');
    let start: u32 = parts.next().and_then(|n| n.parse().ok()).unwrap_or(0);
    let count: u32 = parts.next().and_then(|n| n.parse().ok()).unwrap_or(1);
    let lines = match count {
        0 => format!("After line {start}"),
        1 => format!("Line {start}"),
        _ => format!("Lines {start}–{}", start + count - 1),
    };
    (lines, context)
}

/// Remote branches with no local branch of the same name.
pub(crate) fn remote_only(status: &GitStatus) -> Vec<String> {
    status
        .remote_branches
        .iter()
        .filter(|r| {
            let name = r.split_once('/').map_or(r.as_str(), |(_, n)| n);
            !status.branches.iter().any(|b| b == name)
        })
        .cloned()
        .collect()
}

/// Branches that can be merged into the one checked out: the other local
/// ones, then the remote ones.
pub(crate) fn mergeable(status: &GitStatus) -> Vec<String> {
    status
        .branches
        .iter()
        .filter(|b| Some(b.as_str()) != status.branch.as_deref())
        .chain(status.remote_branches.iter())
        .cloned()
        .collect()
}

/// The working tree's row over the commits, as a commit whose parent is HEAD.
pub(crate) const WORKTREE: &str = "worktree";

/// A line in the graph from one row to the next, by lane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct GraphEdge {
    pub from: usize,
    pub to: usize,
    /// The lane whose colour it takes.
    pub color: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GraphRow {
    pub lane: usize,
    /// Lines from this row's middle to the next row's.
    pub edges: Vec<GraphEdge>,
}

/// Lays out the commit graph: each commit in a lane, newest first, with the
/// lines to its parents. A lane waits for one commit; a commit takes the
/// first lane waiting for it (others waiting for it end there, as branches
/// that forked from it), its first parent continues its lane, and a merge's
/// other parents join a lane already waiting for them or start a new one.
/// Lanes never move, so a branch is a straight line.
pub(crate) fn layout_graph(commits: &[(String, Vec<String>)]) -> (Vec<GraphRow>, usize) {
    struct Row {
        lane: usize,
        after: Vec<Option<String>>,
        through: Vec<usize>,
        started: Vec<usize>,
    }
    let mut lanes: Vec<Option<String>> = Vec::new();
    let mut rows: Vec<Row> = Vec::new();
    let mut width = 0;
    let free = |lanes: &Vec<Option<String>>| {
        lanes
            .iter()
            .position(Option::is_none)
            .unwrap_or(lanes.len())
    };
    for (sha, parents) in commits {
        let waiting: Vec<usize> = lanes
            .iter()
            .enumerate()
            .filter(|(_, s)| s.as_deref() == Some(sha.as_str()))
            .map(|(i, _)| i)
            .collect();
        let lane = waiting.first().copied().unwrap_or_else(|| free(&lanes));
        let through: Vec<usize> = lanes
            .iter()
            .enumerate()
            .filter(|(_, s)| s.as_ref().is_some_and(|s| s != sha))
            .map(|(i, _)| i)
            .collect();
        for &i in &waiting {
            lanes[i] = None;
        }
        let mut started = Vec::new();
        if lane >= lanes.len() {
            lanes.resize(lane + 1, None);
        }
        lanes[lane] = parents.first().cloned();
        if !parents.is_empty() {
            started.push(lane);
        }
        for parent in parents.iter().skip(1) {
            let at = match lanes
                .iter()
                .position(|s| s.as_deref() == Some(parent.as_str()))
            {
                Some(at) => at,
                None => {
                    let at = free(&lanes);
                    if at >= lanes.len() {
                        lanes.resize(at + 1, None);
                    }
                    lanes[at] = Some(parent.clone());
                    at
                }
            };
            started.push(at);
        }
        while lanes.last().is_some_and(Option::is_none) {
            lanes.pop();
        }
        width = width.max(lanes.len()).max(lane + 1);
        rows.push(Row {
            lane,
            after: lanes.clone(),
            through,
            started,
        });
    }
    let out = (0..rows.len())
        .map(|i| {
            let row = &rows[i];
            let next = commits.get(i + 1);
            let mut edges = Vec::new();
            for (k, sha) in row.after.iter().enumerate() {
                let Some(sha) = sha else { continue };
                let to = match next {
                    Some((next_sha, _)) if next_sha == sha => rows[i + 1].lane,
                    _ => k,
                };
                if row.through.contains(&k) {
                    edges.push(GraphEdge {
                        from: k,
                        to,
                        color: k,
                    });
                }
                if row.started.contains(&k) {
                    edges.push(GraphEdge {
                        from: row.lane,
                        to,
                        color: k,
                    });
                }
            }
            GraphRow {
                lane: row.lane,
                edges,
            }
        })
        .collect();
    (out, width)
}

/// Lane colours, as in the mockup: blue, then violet, and on.
pub(crate) const LANE_COLORS: [u32; 6] =
    [0x4b8df0, 0xa274e8, 0x2fb39a, 0xe0843c, 0xd9567a, 0x8a9a2a];

pub(crate) struct Avatar {
    pub initials: String,
    /// A gradient, top-left to bottom-right.
    pub colors: (u32, u32),
    /// Who the tooltip names: "Huan Yang", or "Huan Yang with Claude Code".
    pub who: String,
}

const PEOPLE: [(u32, u32); 6] = [
    (0x2aa6c9, 0x2fc7a0),
    (0xe2a33c, 0xd9672f),
    (0x5aa469, 0x2e8b7a),
    (0xc75bb0, 0x7b6cf0),
    (0x3f8cf0, 0x2aa6c9),
    (0xd9467a, 0xe2a33c),
];
const CLAUDE: (u32, u32) = (0xe0743c, 0xd9467a);
const CODEX: (u32, u32) = (0x7b6cf0, 0x3f8cf0);

/// Which agent helped write a commit, from its author and co-authors.
pub(crate) fn commit_agent(
    author: &str,
    email: &str,
    coauthors: &[String],
) -> Option<&'static str> {
    let people: Vec<String> = std::iter::once(format!("{author} <{email}>"))
        .chain(coauthors.iter().cloned())
        .map(|p| p.to_lowercase())
        .collect();
    if people
        .iter()
        .any(|p| p.contains("noreply@anthropic.com") || p.starts_with("claude"))
    {
        return Some("claude");
    }
    if people.iter().any(|p| {
        p.split(|c: char| !c.is_alphanumeric())
            .any(|w| w == "codex")
            || p.contains("@openai.com")
    }) {
        return Some("codex");
    }
    None
}

/// A commit's avatar: the author's initials, or Claude Code's or Codex's.
pub(crate) fn avatar(author: &str, email: &str, coauthors: &[String]) -> Avatar {
    let name = if author.trim().is_empty() {
        email.to_string()
    } else {
        author.trim().to_string()
    };
    if let Some(agent) = commit_agent(author, email, coauthors) {
        let label = if agent == "claude" {
            "Claude Code"
        } else {
            "Codex"
        };
        let human = !name.to_lowercase().contains(agent);
        return Avatar {
            initials: if agent == "claude" { "CC" } else { "CX" }.into(),
            colors: if agent == "claude" { CLAUDE } else { CODEX },
            who: if human {
                format!("{name} with {label}")
            } else {
                label.into()
            },
        };
    }
    let words: Vec<&str> = name
        .split(|c: char| c.is_whitespace() || "._-".contains(c))
        .filter(|w| !w.is_empty())
        .collect();
    let first = |w: &str| w.chars().next().unwrap_or('?');
    let initials: String = match words.as_slice() {
        [a, b, ..] => [first(a), first(b)].iter().collect(),
        [a] => first(a).to_string(),
        [] => "?".into(),
    }
    .to_uppercase();
    let key = if email.is_empty() {
        name.clone()
    } else {
        email.to_lowercase()
    };
    let mut hash: i32 = 0;
    for c in key.encode_utf16() {
        hash = hash.wrapping_mul(31).wrapping_add(c as i32);
    }
    Avatar {
        initials,
        colors: PEOPLE[hash.unsigned_abs() as usize % PEOPLE.len()],
        who: name,
    }
}

/// The co-author trailers of a whole message.
pub(crate) fn coauthors_of(message: &str) -> Vec<String> {
    message
        .lines()
        .filter_map(|l| {
            let lower = l.to_lowercase();
            lower
                .strip_prefix("co-authored-by:")
                .map(|_| l["co-authored-by:".len()..].trim().to_string())
        })
        .collect()
}

/// "now", "5m", "3h", "2d", then the date: how long ago, for a commit row.
pub(crate) fn time_ago(ms: i64, now: i64) -> String {
    let s = ((now - ms) / 1000).max(0);
    if s < 60 {
        return "now".into();
    }
    if s < 3600 {
        return format!("{}m", s / 60);
    }
    if s < 86400 {
        return format!("{}h", s / 3600);
    }
    if s < 7 * 86400 {
        return format!("{}d", s / 86400);
    }
    use chrono::{Datelike, TimeZone};
    let (Some(then), Some(today)) = (
        chrono::Local.timestamp_millis_opt(ms).single(),
        chrono::Local.timestamp_millis_opt(now).single(),
    ) else {
        return String::new();
    };
    if then.year() == today.year() {
        then.format("%b %-d").to_string()
    } else {
        then.format("%b %Y").to_string()
    }
}

/// A full date and time, for a commit's details.
pub(crate) fn when(ms: i64) -> String {
    use chrono::TimeZone;
    chrono::Local
        .timestamp_millis_opt(ms)
        .single()
        .map(|t| t.format("%b %-d, %Y, %H:%M").to_string())
        .unwrap_or_default()
}

/// A commit's refs as badges: a local branch also on its remote shows the
/// globe, once.
pub(crate) struct Badge {
    pub kind: String,
    pub name: String,
    pub remote: bool,
}

pub(crate) fn badges(commit: &GitCommit) -> Vec<Badge> {
    let local: Vec<&str> = commit
        .refs
        .iter()
        .filter(|r| r.kind == "head" || r.kind == "branch")
        .map(|r| r.name.as_str())
        .collect();
    let mut out = Vec::new();
    for r in &commit.refs {
        if r.kind == "remote" {
            let name = r.name.split_once('/').map_or(r.name.as_str(), |(_, n)| n);
            if local.contains(&name) {
                continue;
            }
            out.push(Badge {
                kind: "remote".into(),
                name: r.name.clone(),
                remote: false,
            });
        } else {
            let remote = r.kind != "tag"
                && commit
                    .refs
                    .iter()
                    .any(|o| o.kind == "remote" && o.name.ends_with(&format!("/{}", r.name)));
            out.push(Badge {
                kind: r.kind.clone(),
                name: r.name.clone(),
                remote,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{
        DiffKind, avatar, branch_name, commit_message, diff_lines, hunk_label, layout_graph,
        split_message, split_path,
    };

    #[test]
    fn diffs_read_with_line_numbers() {
        let text = "@@ -3,3 +3,4 @@ fn main\n a\n-b\n+c\n+d\n e\n";
        let lines = diff_lines(text);
        let kinds: Vec<DiffKind> = lines.iter().map(|l| l.kind).collect();
        assert_eq!(
            kinds,
            [
                DiffKind::Hunk,
                DiffKind::Context,
                DiffKind::Del,
                DiffKind::Add,
                DiffKind::Add,
                DiffKind::Context
            ]
        );
        assert_eq!((lines[1].old, lines[1].new), (Some(3), Some(3)));
        assert_eq!(lines[2].old, Some(4));
        assert_eq!(lines[4].new, Some(5));
        assert_eq!(lines[4].at, 4);
        assert_eq!(
            hunk_label("@@ -3,3 +3,4 @@ fn main"),
            ("Lines 3–6".to_string(), "fn main".to_string())
        );
        assert_eq!(hunk_label("@@ -1 +1 @@").0, "Line 1");
    }

    #[test]
    fn messages_and_names() {
        assert_eq!(commit_message(" Fix it ", ""), "Fix it");
        assert_eq!(commit_message("Fix", "Why\n"), "Fix\n\nWhy");
        assert_eq!(split_message("Fix\n\nWhy"), ("Fix".into(), "Why".into()));
        assert_eq!(branch_name(" feature/qc "), Some("feature/qc".into()));
        assert_eq!(branch_name("bad name"), None);
        assert_eq!(branch_name("a..b"), None);
        assert_eq!(
            split_path("scripts/qc.R"),
            ("qc.R".into(), "scripts".into())
        );
        assert_eq!(split_path("data/"), ("data/".into(), String::new()));
    }

    #[test]
    fn graph_and_avatars() {
        // c merges b into a; b and a both come from root.
        let commits = vec![
            ("c".to_string(), vec!["a".to_string(), "b".to_string()]),
            ("b".to_string(), vec!["r".to_string()]),
            ("a".to_string(), vec!["r".to_string()]),
            ("r".to_string(), vec![]),
        ];
        let (rows, width) = layout_graph(&commits);
        assert_eq!(width, 2);
        assert_eq!(
            rows.iter().map(|r| r.lane).collect::<Vec<_>>(),
            [0, 1, 0, 0]
        );
        let claude = avatar(
            "Huan Yang",
            "h@x.org",
            &["Claude <noreply@anthropic.com>".into()],
        );
        assert_eq!(claude.initials, "CC");
        assert_eq!(claude.who, "Huan Yang with Claude Code");
        assert_eq!(avatar("Huan Yang", "h@x.org", &[]).initials, "HY");
    }
}
