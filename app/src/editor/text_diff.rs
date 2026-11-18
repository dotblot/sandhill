//! Comparing two texts line by line: an agent's change to a cell, a cell's
//! code against the code that made its outputs, a file against an earlier
//! version of it.

use std::time::Duration;

use similar::{Algorithm, ChangeTag, TextDiff};

/// Unchanged lines shown around a change.
const CONTEXT_LINES: usize = 2;
/// How long a comparison may take before a rougher one is shown; long files
/// that differ everywhere would otherwise hold up drawing.
const DEADLINE: Duration = Duration::from_millis(500);

/// One line of a comparison.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum DiffLine {
    Same(String),
    Removed(String),
    Added(String),
    /// Unchanged lines left out.
    Gap(usize),
}

/// The lines of `before` and `after`, removed, added or the same, with
/// long runs of unchanged lines left out. Removed lines come before the
/// lines that replace them.
pub(crate) fn diff(before: &str, after: &str) -> Vec<DiffLine> {
    let old: Vec<&str> = before.lines().collect();
    let new: Vec<&str> = after.lines().collect();
    let compared = TextDiff::configure()
        .algorithm(Algorithm::Myers)
        .timeout(DEADLINE)
        .diff_slices(&old, &new);
    let lines = compared
        .iter_all_changes()
        .map(|change| {
            let text = change.value().to_string();
            match change.tag() {
                ChangeTag::Equal => DiffLine::Same(text),
                ChangeTag::Delete => DiffLine::Removed(text),
                ChangeTag::Insert => DiffLine::Added(text),
            }
        })
        .collect();
    fold_unchanged(lines)
}

/// Leaves out unchanged lines further than [`CONTEXT_LINES`] from a change.
fn fold_unchanged(lines: Vec<DiffLine>) -> Vec<DiffLine> {
    let changed: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| !matches!(l, DiffLine::Same(_)))
        .map(|(i, _)| i)
        .collect();
    // `changed` is in order, so the nearest change is found by bisection.
    let near = |i: usize| {
        let at = changed.partition_point(|&c| c < i);
        let after = changed.get(at).is_some_and(|&c| c - i <= CONTEXT_LINES);
        let before = at > 0 && i - changed[at - 1] <= CONTEXT_LINES;
        after || before
    };
    let mut folded = Vec::new();
    let mut gap = 0;
    for (i, line) in lines.into_iter().enumerate() {
        if matches!(line, DiffLine::Same(_)) && !near(i) {
            gap += 1;
            continue;
        }
        if gap > 0 {
            folded.push(DiffLine::Gap(gap));
            gap = 0;
        }
        folded.push(line);
    }
    if gap > 0 {
        folded.push(DiffLine::Gap(gap));
    }
    folded
}

#[cfg(test)]
mod tests {
    use super::{DiffLine, diff};

    #[test]
    fn shows_what_changed() {
        use DiffLine::*;
        assert_eq!(
            diff("a\nb\nc", "a\nB\nc\nd"),
            vec![
                Same("a".into()),
                Removed("b".into()),
                Added("B".into()),
                Same("c".into()),
                Added("d".into()),
            ]
        );
        // Unchanged lines far from a change are left out.
        let before = "1\n2\n3\n4\n5\n6\n7\n8";
        let after = "1\n2\n3\n4\n5\n6\n7\nX";
        assert_eq!(
            diff(before, after),
            vec![
                Gap(5),
                Same("6".into()),
                Same("7".into()),
                Removed("8".into()),
                Added("X".into()),
            ]
        );
    }

    #[test]
    fn a_long_file_with_one_change_shows_just_that_change() {
        let before: String = (0..50_000).map(|i| format!("line {i}\n")).collect();
        let after = before.replace("line 25000\n", "line twenty-five thousand\n");
        let lines = diff(&before, &after);
        assert_eq!(lines.len(), 8, "{lines:?}");
        assert_eq!(lines[3], DiffLine::Removed("line 25000".into()));
        assert_eq!(
            lines[4],
            DiffLine::Added("line twenty-five thousand".into())
        );
    }
}
