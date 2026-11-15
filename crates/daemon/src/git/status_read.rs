use super::*;

/// Reads `git status --porcelain=v2 --branch -z`.
pub(super) fn parse_status(text: &str) -> GitStatus {
    let mut status = GitStatus::default();
    let mut items = text.split('\0');
    let letter = |c: char| (c != '.').then(|| c.to_string());
    while let Some(item) = items.next() {
        if let Some(header) = item.strip_prefix("# ") {
            let (key, value) = header.split_once(' ').unwrap_or((header, ""));
            match key {
                "branch.oid" if value != "(initial)" => status.head = Some(value.to_string()),
                "branch.head" if value != "(detached)" => status.branch = Some(value.to_string()),
                "branch.upstream" => status.upstream = Some(value.to_string()),
                "branch.ab" => {
                    for part in value.split(' ') {
                        if let Some(n) = part.strip_prefix('+') {
                            status.ahead = n.parse().unwrap_or(0);
                        } else if let Some(n) = part.strip_prefix('-') {
                            status.behind = n.parse().unwrap_or(0);
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        let mut file = match item.split_at_checked(2) {
            Some(("1 ", rest)) => {
                let fields: Vec<&str> = rest.splitn(8, ' ').collect();
                let [xy, .., path] = fields.as_slice() else {
                    continue;
                };
                changed(xy, path, &letter)
            }
            Some(("2 ", rest)) => {
                let fields: Vec<&str> = rest.splitn(9, ' ').collect();
                let [xy, .., path] = fields.as_slice() else {
                    continue;
                };
                let mut file = changed(xy, path, &letter);
                file.from = items.next().map(str::to_string);
                file
            }
            Some(("u ", rest)) => {
                let fields: Vec<&str> = rest.splitn(10, ' ').collect();
                let Some(path) = fields.last() else {
                    continue;
                };
                GitFile {
                    path: path.to_string(),
                    conflict: true,
                    unstaged: Some("U".into()),
                    ..GitFile::default()
                }
            }
            Some(("? ", path)) => GitFile {
                path: path.to_string(),
                unstaged: Some("?".into()),
                ..GitFile::default()
            },
            _ => continue,
        };
        if status.files.len() >= MAX_FILES {
            status.more = true;
            continue;
        }
        file.path.shrink_to_fit();
        status.files.push(file);
    }
    status.files.sort_by(|a, b| a.path.cmp(&b.path));
    status
}

fn changed(xy: &str, path: &str, letter: &impl Fn(char) -> Option<String>) -> GitFile {
    let mut chars = xy.chars();
    GitFile {
        path: path.to_string(),
        staged: chars.next().and_then(letter),
        unstaged: chars.next().and_then(letter),
        ..GitFile::default()
    }
}

/// Reads `git diff --numstat -z`: lines added and removed by path (a renamed
/// file's new path).
pub(super) fn parse_numstat(text: &str) -> HashMap<String, GitLines> {
    let mut lines = HashMap::new();
    let mut items = text.split('\0');
    while let Some(item) = items.next() {
        let mut fields = item.splitn(3, '\t');
        let (Some(added), Some(removed), Some(path)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let path = if path.is_empty() {
            // A rename: its old and new paths follow.
            items.next();
            match items.next() {
                Some(path) => path,
                None => break,
            }
        } else {
            path
        };
        let binary = added == "-";
        lines.insert(
            path.to_string(),
            GitLines {
                added: added.parse().unwrap_or(0),
                removed: removed.parse().unwrap_or(0),
                binary,
            },
        );
    }
    lines
}

/// An untracked file's lines, when it is small enough to read; binary when
/// it has a NUL byte.
pub(super) fn count_lines(path: &Path) -> Option<GitLines> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > COUNT_MAX {
        return None;
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes[..bytes.len().min(8000)].contains(&0) {
        return Some(GitLines {
            binary: true,
            ..GitLines::default()
        });
    }
    let newlines = bytes.iter().filter(|&&b| b == b'\n').count();
    let last = !bytes.is_empty() && !bytes.ends_with(b"\n");
    Some(GitLines {
        added: (newlines + usize::from(last)) as u32,
        ..GitLines::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_status() {
        let text = "# branch.oid 1234abcd\0# branch.head main\0# branch.upstream origin/main\0\
# branch.ab +3 -1\0\
1 M. N... 100644 100644 100644 aaa bbb analysis.qmd\0\
1 .M N... 100644 100644 100644 aaa bbb R/01 qc.R\0\
2 R. N... 100644 100644 100644 aaa bbb R100 new.R\0old.R\0\
u UU N... 100644 100644 100644 100644 a b c conflict.R\0\
? CLAUDE.md\0? data/\0";
        let status = parse_status(text);
        assert_eq!(status.head.as_deref(), Some("1234abcd"));
        assert_eq!(status.branch.as_deref(), Some("main"));
        assert_eq!(status.upstream.as_deref(), Some("origin/main"));
        assert_eq!((status.ahead, status.behind), (3, 1));
        let summary: Vec<_> = status
            .files
            .iter()
            .map(|f| {
                (
                    f.path.as_str(),
                    f.staged.as_deref(),
                    f.unstaged.as_deref(),
                    f.from.as_deref(),
                    f.conflict,
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                ("CLAUDE.md", None, Some("?"), None, false),
                ("R/01 qc.R", None, Some("M"), None, false),
                ("analysis.qmd", Some("M"), None, None, false),
                ("conflict.R", None, Some("U"), None, true),
                ("data/", None, Some("?"), None, false),
                ("new.R", Some("R"), None, Some("old.R"), false),
            ]
        );
    }

    #[test]
    fn reads_a_new_repository_and_a_detached_head() {
        let status = parse_status("# branch.oid (initial)\0# branch.head main\0? a.R\0");
        assert_eq!(status.head, None);
        assert_eq!(status.branch.as_deref(), Some("main"));
        let status = parse_status("# branch.oid abc\0# branch.head (detached)\0");
        assert_eq!(status.branch, None);
        assert_eq!(status.head.as_deref(), Some("abc"));
    }

    #[test]
    fn reads_numstat_with_renames_and_binaries() {
        let lines = parse_numstat("12\t0\tanalysis.qmd\0-\t-\tumap.png\x003\t1\t\0old.R\0new.R\0");
        assert_eq!(lines["analysis.qmd"].added, 12);
        assert!(lines["umap.png"].binary);
        assert_eq!((lines["new.R"].added, lines["new.R"].removed), (3, 1));
        assert!(!lines.contains_key("old.R"));
    }
}
