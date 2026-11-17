//! The grid's horizontal window, in display positions (not kernel column indices).
use std::collections::BTreeSet;
use std::ops::Range;

#[derive(Debug, PartialEq)]
pub(super) struct ColumnViewport {
    pub pinned: Range<usize>,
    pub scrolling: Range<usize>,
    pub before: f32,
}

pub(super) fn column_viewport(
    columns: &[u32],
    widths: &[f32],
    pinned: usize,
    offset: f32,
    viewport: f32,
    label: f32,
) -> ColumnViewport {
    let width = |i: usize| widths.get(columns[i] as usize).copied().unwrap_or(140.);
    let mut fixed = label;
    let mut shown_pinned = 0;
    for i in 0..pinned {
        if fixed < viewport {
            shown_pinned = i + 1;
        }
        fixed += width(i);
    }
    let mut start = pinned;
    let mut before = 0.;
    while start < columns.len() && before + width(start) <= offset {
        before += width(start);
        start += 1;
    }
    let mut end = start;
    let mut right = before;
    let available = (viewport - fixed).max(0.);
    while end < columns.len() && available > 0. && right < offset + available {
        right += width(end);
        end += 1;
    }
    ColumnViewport {
        pinned: 0..shown_pinned,
        scrolling: start..end,
        before,
    }
}

/// Pinned columns keep their explicit order; the rest keep their original
/// order. Membership checks must not scan the hidden or pinned lists for
/// each column, especially after Hide All on a wide table.
pub(super) fn display_columns(count: u32, hidden: &BTreeSet<u32>, pinned: &[u32]) -> Vec<u32> {
    let pinned_set: BTreeSet<u32> = pinned.iter().copied().collect();
    let shown = |i: &u32| *i < count && !hidden.contains(i);
    pinned
        .iter()
        .copied()
        .filter(shown)
        .chain((0..count).filter(|i| shown(i) && !pinned_set.contains(i)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_table_only_builds_columns_in_the_viewport() {
        let columns: Vec<u32> = (0..32_738).collect();
        let widths = vec![100.; columns.len()];
        assert_eq!(
            column_viewport(&columns, &widths, 0, 0., 860., 60.),
            ColumnViewport {
                pinned: 0..0,
                scrolling: 0..8,
                before: 0.
            }
        );
        assert_eq!(
            column_viewport(&columns, &widths, 2, 10_000., 860., 60.),
            ColumnViewport {
                pinned: 0..2,
                scrolling: 102..108,
                before: 10_000.
            }
        );
        assert_eq!(
            column_viewport(&columns, &widths, 1000, 0., 860., 60.),
            ColumnViewport {
                pinned: 0..8,
                scrolling: 1000..1000,
                before: 0.
            }
        );
    }

    #[test]
    fn wide_table_projection_preserves_hidden_pinned_and_partial_columns() {
        let columns = [4, 1, 3, 0];
        let widths = [50., 100., 900., 70., 120.];
        assert_eq!(
            column_viewport(&columns, &widths, 1, 85., 310., 60.),
            ColumnViewport {
                pinned: 0..1,
                scrolling: 1..4,
                before: 0.
            }
        );
        assert_eq!(
            column_viewport(&[], &widths, 0, 0., 310., 60.),
            ColumnViewport {
                pinned: 0..0,
                scrolling: 0..0,
                before: 0.
            }
        );
    }

    #[test]
    fn many_hidden_columns_keep_pinned_and_original_order_when_shown_again() {
        let count = 32_738;
        let pinned = [32_737, 2, 32_738, 0];
        let mut hidden: BTreeSet<u32> = (0..count).collect();
        assert!(display_columns(count, &hidden, &pinned).is_empty());
        for column in [0, 1, 2, 17_000, 32_737] {
            hidden.remove(&column);
        }
        assert_eq!(
            display_columns(count, &hidden, &pinned),
            vec![32_737, 2, 0, 1, 17_000]
        );
        hidden.insert(2);
        assert_eq!(
            display_columns(count, &hidden, &pinned),
            vec![32_737, 0, 1, 17_000]
        );
        hidden.clear();
        let shown = display_columns(count, &hidden, &pinned);
        assert_eq!(&shown[..3], &[32_737, 2, 0]);
        assert_eq!(shown.len(), count as usize);
        assert!(shown[3..].windows(2).all(|pair| pair[0] < pair[1]));
    }
}
