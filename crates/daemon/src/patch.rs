//! Part of a file's diff, as a patch `git apply` takes: to stage, unstage or
//! throw away some of its lines, as `git add -p` does. The lines are picked
//! from the diff the app showed (`git_diff`'s text), so git checks that the
//! file is still as shown: a patch that doesn't fit is refused, not applied
//! somewhere else.

use std::collections::BTreeSet;
use std::ops::Range;

use anyhow::{Result, bail};

/// Which way the patch goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Way {
    /// The picked changes go forward: staging from the unstaged diff.
    Forward,
    /// The picked changes are undone: unstaging from the staged diff, or
    /// throwing away from the unstaged one.
    Reverse,
}

/// The patch for `file` that holds only the `picked` lines of `diff` (by
/// their place among its lines, counting from 0). Applied the `way` it is
/// made for (`git apply`, with `--reverse` for [`Way::Reverse`]), it makes
/// those lines' changes and leaves the rest as they are.
pub fn partial(file: &str, diff: &str, picked: &BTreeSet<usize>, way: Way) -> Result<String> {
    let lines: Vec<&str> = diff.split('\n').collect();
    let mut hunks = String::new();
    // How far the lines git applies to have moved, from earlier hunks.
    let mut moved = 0;
    let mut at = 0;
    while at < lines.len() {
        let Some((old_start, old_count, new_start, new_count)) = hunk_header(lines[at]) else {
            // Only the diff's last newline is outside a hunk.
            if lines[at].is_empty() && at == lines.len() - 1 {
                break;
            }
            bail!("this isn't a diff git made: {:?}", lines[at]);
        };
        let end = lines[at + 1..]
            .iter()
            .position(|line| line.starts_with("@@"))
            .map_or(lines.len(), |i| at + 1 + i);
        let range = at + 1..end;
        at = end;
        let Some(Hunk { body, old, new, .. }) = hunk(&lines, range, picked, way)? else {
            continue;
        };
        // Git finds a hunk by the side it applies to, which is as the diff
        // has it: the old side going forward, the new side going back. The
        // other side's start is where the hunk lands after the earlier ones.
        let (old_start, new_start) = match way {
            Way::Forward => {
                let first = first_line(old_start, old_count);
                (header_start(first, old), header_start(first + moved, new))
            }
            Way::Reverse => {
                let first = first_line(new_start, new_count);
                (header_start(first + moved, old), header_start(first, new))
            }
        };
        moved += match way {
            Way::Forward => new - old,
            Way::Reverse => old - new,
        };
        hunks.push_str(&format!(
            "@@ -{old_start},{old} +{new_start},{new} @@\n{body}"
        ));
    }
    if hunks.is_empty() {
        bail!("pick lines that are added or removed");
    }
    let (a, b) = (side("a", file), side("b", file));
    Ok(format!("diff --git {a} {b}\n--- {a}\n+++ {b}\n{hunks}"))
}

/// A hunk of the patch: its lines, and how many each side has.
#[derive(Default)]
struct Hunk {
    body: String,
    old: i64,
    new: i64,
    changed: bool,
    // Sides that reached the end of a file with no newline.
    old_ended: bool,
    new_ended: bool,
}

/// A line of the diff, with the "\ No newline at end of file" after it.
struct Line<'a> {
    at: usize,
    mark: u8,
    text: &'a str,
    note: Option<&'a str>,
}

impl Hunk {
    fn push(&mut self, mark: char, line: &Line) -> Result<()> {
        let (old, new) = (mark != '+', mark != '-');
        if (old && self.old_ended) || (new && self.new_ended) {
            // A line after the file's last one: the change to that line
            // has to come too.
            bail!("pick the change to the last line too: it has no newline at its end");
        }
        self.changed |= mark != ' ';
        self.old += i64::from(old);
        self.new += i64::from(new);
        self.body.push(mark);
        self.body.push_str(line.text);
        self.body.push('\n');
        if let Some(note) = line.note {
            self.old_ended |= old;
            self.new_ended |= new;
            self.body.push_str(note);
            self.body.push('\n');
        }
        Ok(())
    }
}

/// The hunk made from the diff's `lines` in `range`, or none when no change
/// in it is picked.
fn hunk(
    lines: &[&str],
    range: Range<usize>,
    picked: &BTreeSet<usize>,
    way: Way,
) -> Result<Option<Hunk>> {
    let mut read: Vec<Line> = Vec::new();
    for at in range {
        let line = lines[at];
        match line.as_bytes().first() {
            Some(&mark @ (b' ' | b'-' | b'+')) => read.push(Line {
                at,
                mark,
                text: &line[1..],
                note: None,
            }),
            Some(b'\\') if !read.is_empty() => read.last_mut().unwrap().note = Some(line),
            // The diff's last newline.
            None if at == lines.len() - 1 => {}
            _ => bail!("this isn't a diff git made: {line:?}"),
        }
    }
    let mut hunk = Hunk::default();
    let mut i = 0;
    while i < read.len() {
        if read[i].mark == b' ' {
            hunk.push(' ', &read[i])?;
            i += 1;
            continue;
        }
        // A change: lines removed, then the lines that took their place.
        // Each removed line is paired with the added one in the same place,
        // so a line's change picked on its own lands where the line was.
        let end = read[i..]
            .iter()
            .position(|l| l.mark == b' ')
            .map_or(read.len(), |n| i + n);
        let (removed, added): (Vec<&Line>, Vec<&Line>) =
            read[i..end].iter().partition(|l| l.mark == b'-');
        for n in 0..removed.len().max(added.len()) {
            if let Some(line) = removed.get(n) {
                match (picked.contains(&line.at), way) {
                    (true, _) => hunk.push('-', line)?,
                    // Left out going forward, the line stays; going back, it
                    // stays gone.
                    (false, Way::Forward) => hunk.push(' ', line)?,
                    (false, Way::Reverse) => {}
                }
            }
            if let Some(line) = added.get(n) {
                match (picked.contains(&line.at), way) {
                    (true, _) => hunk.push('+', line)?,
                    // Left out going forward, the line never was; going back,
                    // it stays.
                    (false, Way::Forward) => {}
                    (false, Way::Reverse) => hunk.push(' ', line)?,
                }
            }
        }
        i = end;
    }
    Ok(hunk.changed.then_some(hunk))
}

/// `@@ -a,b +c,d @@` as its numbers; a missing count is 1.
fn hunk_header(line: &str) -> Option<(i64, i64, i64, i64)> {
    let rest = line.strip_prefix("@@ -")?;
    let (ranges, _) = rest.split_once(" @@")?;
    let (old, new) = ranges.split_once(" +")?;
    let range = |r: &str| -> Option<(i64, i64)> {
        match r.split_once(',') {
            Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
            None => Some((r.parse().ok()?, 1)),
        }
    };
    let (a, b) = range(old)?;
    let (c, d) = range(new)?;
    Some((a, b, c, d))
}

/// Where a side's range begins: an empty range names the line before.
fn first_line(start: i64, count: i64) -> i64 {
    if count == 0 { start + 1 } else { start }
}

/// A range's start as a header writes it.
fn header_start(first: i64, count: i64) -> i64 {
    if count == 0 { first - 1 } else { first }
}

/// `a/file` as git's headers write it: in quotes, with C escapes, when it
/// has a quote, a backslash or a control character.
fn side(prefix: &str, file: &str) -> String {
    let special = |b: u8| b == b'"' || b == b'\\' || b < 0x20 || b == 0x7f;
    if !file.bytes().any(special) {
        return format!("{prefix}/{file}");
    }
    let mut out = format!("\"{prefix}/");
    for c in file.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if c.is_ascii() && special(c as u8) => out.push_str(&format!("\\{:03o}", c as u8)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIFF: &str = "@@ -1,4 +1,5 @@\n a\n-b\n+B\n+new\n c\n d\n@@ -10,3 +11,2 @@\n x\n-y\n z\n";

    fn pick(lines: &[usize]) -> BTreeSet<usize> {
        lines.iter().copied().collect()
    }

    fn hunks(patch: &str) -> &str {
        &patch[patch.find("@@").unwrap()..]
    }

    #[test]
    fn staging_keeps_what_is_left_out_as_it_was() {
        // "+new" only: "-b" stays, "+B" never was.
        let patch = partial("f.txt", DIFF, &pick(&[4]), Way::Forward).unwrap();
        assert!(patch.starts_with("diff --git a/f.txt b/f.txt\n--- a/f.txt\n+++ b/f.txt\n@@"));
        assert_eq!(hunks(&patch), "@@ -1,4 +1,5 @@\n a\n b\n+new\n c\n d\n");
    }

    #[test]
    fn later_hunks_land_where_the_earlier_ones_moved_them() {
        // "-b" and "-y": the first hunk loses a line, so the second one is
        // a line higher in the result.
        let patch = partial("f.txt", DIFF, &pick(&[2, 9]), Way::Forward).unwrap();
        assert_eq!(
            hunks(&patch),
            "@@ -1,4 +1,3 @@\n a\n-b\n c\n d\n@@ -10,3 +9,2 @@\n x\n-y\n z\n"
        );
    }

    #[test]
    fn hunks_with_nothing_picked_are_left_out() {
        let patch = partial("f.txt", DIFF, &pick(&[9]), Way::Forward).unwrap();
        assert_eq!(hunks(&patch), "@@ -10,3 +10,2 @@\n x\n-y\n z\n");
        let err = partial("f.txt", DIFF, &pick(&[1, 5]), Way::Forward).unwrap_err();
        assert!(err.to_string().contains("pick lines"), "{err}");
    }

    #[test]
    fn going_back_keeps_the_new_side() {
        // Undoing "+B" only: "+new" stays, as context, and "-b" comes back.
        let patch = partial("f.txt", DIFF, &pick(&[2, 3]), Way::Reverse).unwrap();
        assert_eq!(hunks(&patch), "@@ -1,5 +1,5 @@\n a\n-b\n+B\n new\n c\n d\n");
        // Undoing "+new" only: "-b" is left out, "+B" is context.
        let patch = partial("f.txt", DIFF, &pick(&[4, 9]), Way::Reverse).unwrap();
        assert_eq!(
            hunks(&patch),
            "@@ -1,4 +1,5 @@\n a\n B\n+new\n c\n d\n@@ -10,3 +11,2 @@\n x\n-y\n z\n"
        );
    }

    #[test]
    fn a_line_changed_on_its_own_stays_in_its_place() {
        let diff = "@@ -1,2 +1,3 @@\n-x\n-y\n+X\n+Y\n+z\n";
        let patch = partial("f", diff, &pick(&[1, 3]), Way::Forward).unwrap();
        assert_eq!(hunks(&patch), "@@ -1,2 +1,2 @@\n-x\n+X\n y\n");
        let patch = partial("f", diff, &pick(&[2, 4]), Way::Forward).unwrap();
        assert_eq!(hunks(&patch), "@@ -1,2 +1,2 @@\n x\n-y\n+Y\n");
        let patch = partial("f", diff, &pick(&[1, 3]), Way::Reverse).unwrap();
        assert_eq!(hunks(&patch), "@@ -1,3 +1,3 @@\n-x\n+X\n Y\n z\n");
    }

    #[test]
    fn empty_sides_name_the_line_before() {
        // Adding to an empty file, and taking all of a file's lines away.
        let added = "@@ -0,0 +1,2 @@\n+a\n+b\n";
        let patch = partial("f", added, &pick(&[2]), Way::Forward).unwrap();
        assert_eq!(hunks(&patch), "@@ -0,0 +1,1 @@\n+b\n");
        let patch = partial("f", added, &pick(&[1, 2]), Way::Reverse).unwrap();
        assert_eq!(hunks(&patch), "@@ -0,0 +1,2 @@\n+a\n+b\n");
        let patch = partial("f", added, &pick(&[1]), Way::Reverse).unwrap();
        assert_eq!(hunks(&patch), "@@ -1,1 +1,2 @@\n+a\n b\n");
        let removed = "@@ -1,2 +0,0 @@\n-a\n-b\n";
        let patch = partial("f", removed, &pick(&[1, 2]), Way::Forward).unwrap();
        assert_eq!(hunks(&patch), "@@ -1,2 +0,0 @@\n-a\n-b\n");
        let patch = partial("f", removed, &pick(&[2]), Way::Reverse).unwrap();
        assert_eq!(hunks(&patch), "@@ -1,1 +0,0 @@\n-b\n");
    }

    #[test]
    fn a_last_line_with_no_newline_comes_with_its_change() {
        let diff = "@@ -1 +1,2 @@\n-foo\n\\ No newline at end of file\n+foo\n+bar\n\\ No newline at end of file\n";
        let err = partial("f", diff, &pick(&[4]), Way::Forward).unwrap_err();
        assert!(err.to_string().contains("last line"), "{err}");
        let patch = partial("f", diff, &pick(&[1, 3, 4]), Way::Forward).unwrap();
        assert_eq!(hunks(&patch), &diff.replace("-1 +1,2", "-1,1 +1,2"));
        // Unstaging "bar" keeps the newline foo gained.
        let patch = partial("f", diff, &pick(&[4]), Way::Reverse).unwrap();
        assert_eq!(
            hunks(&patch),
            "@@ -1,1 +1,2 @@\n foo\n+bar\n\\ No newline at end of file\n"
        );
    }

    #[test]
    fn odd_names_are_quoted_as_git_quotes_them() {
        assert_eq!(side("a", "my file.R"), "a/my file.R");
        assert_eq!(side("a", "résumé.qmd"), "a/résumé.qmd");
        assert_eq!(side("b", "say \"hi\"\t.txt"), "\"b/say \\\"hi\\\"\\t.txt\"");
        assert_eq!(side("a", "x\u{1}"), "\"a/x\\001\"");
    }

    #[test]
    fn diffs_git_did_not_make_are_refused() {
        assert!(partial("f", "hello\n", &pick(&[0]), Way::Forward).is_err());
        assert!(partial("f", "@@ -1 +1 @@\n?x\n", &pick(&[1]), Way::Forward).is_err());
    }
}
