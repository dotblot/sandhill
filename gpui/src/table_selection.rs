//! Rectangular table selection in displayed row and column coordinates.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Target {
    Cell { row: u64, column: usize },
    Row(u64),
    Column(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Range {
    pub first_row: u64,
    pub last_row: u64,
    pub first_column: usize,
    pub last_column: usize,
}

impl Range {
    pub(crate) fn contains(self, row: u64, column: usize) -> bool {
        (self.first_row..=self.last_row).contains(&row)
            && (self.first_column..=self.last_column).contains(&column)
    }

    pub(crate) fn row_count(self) -> u64 {
        self.last_row - self.first_row + 1
    }

    pub(crate) fn column_count(self) -> usize {
        self.last_column - self.first_column + 1
    }
}

impl Target {
    fn bounds(self, rows: u64, columns: usize) -> Range {
        let last_row = rows - 1;
        let last_column = columns - 1;
        match self {
            Self::Cell { row, column } => Range {
                first_row: row.min(last_row),
                last_row: row.min(last_row),
                first_column: column.min(last_column),
                last_column: column.min(last_column),
            },
            Self::Row(row) => Range {
                first_row: row.min(last_row),
                last_row: row.min(last_row),
                first_column: 0,
                last_column,
            },
            Self::Column(column) => Range {
                first_row: 0,
                last_row,
                first_column: column.min(last_column),
                last_column: column.min(last_column),
            },
        }
    }
}

#[derive(Default)]
pub(crate) struct Selection {
    anchor: Option<Target>,
    range: Option<Range>,
    dragging: bool,
}

impl Selection {
    pub(crate) fn begin(&mut self, target: Target, shift: bool, rows: u64, columns: usize) {
        if rows == 0 || columns == 0 {
            self.clear();
            return;
        }
        if !shift || self.anchor.is_none() {
            self.anchor = Some(target);
        }
        self.dragging = true;
        self.extend(target, rows, columns);
    }

    pub(crate) fn extend(&mut self, target: Target, rows: u64, columns: usize) {
        if rows == 0 || columns == 0 {
            self.clear();
            return;
        }
        let Some(anchor) = self.anchor else {
            return;
        };
        let anchor = anchor.bounds(rows, columns);
        let target = target.bounds(rows, columns);
        self.range = Some(Range {
            first_row: anchor.first_row.min(target.first_row),
            last_row: anchor.last_row.max(target.last_row),
            first_column: anchor.first_column.min(target.first_column),
            last_column: anchor.last_column.max(target.last_column),
        });
    }

    pub(crate) fn end(&mut self) {
        self.dragging = false;
    }

    pub(crate) fn clear(&mut self) {
        self.anchor = None;
        self.range = None;
        self.dragging = false;
    }

    pub(crate) fn all(&mut self, rows: u64, columns: usize) {
        if rows == 0 || columns == 0 {
            self.clear();
            return;
        }
        self.begin(Target::Cell { row: 0, column: 0 }, false, rows, columns);
        self.extend(
            Target::Cell {
                row: rows - 1,
                column: columns - 1,
            },
            rows,
            columns,
        );
        self.end();
    }

    pub(crate) fn range(&self) -> Option<Range> {
        self.range
    }

    pub(crate) fn dragging(&self) -> bool {
        self.dragging
    }
}

/// Clipboard TSV, without headers or a trailing newline. Borrowed cell strings
/// are accepted so copying a rectangle does not allocate an intermediate grid.
pub(crate) fn encode_tsv<R, C, S>(rows: R) -> String
where
    R: IntoIterator<Item = C>,
    C: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut text = String::new();
    for (row_index, row) in rows.into_iter().enumerate() {
        if row_index != 0 {
            text.push('\n');
        }
        for (column_index, cell) in row.into_iter().enumerate() {
            if column_index != 0 {
                text.push('\t');
            }
            let cell = cell.as_ref();
            if cell.contains(['\t', '\n', '\r', '"']) {
                text.push('"');
                for ch in cell.chars() {
                    if ch == '"' {
                        text.push('"');
                    }
                    text.push(ch);
                }
                text.push('"');
            } else {
                text.push_str(cell);
            }
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(row: u64, column: usize) -> Target {
        Target::Cell { row, column }
    }

    #[test]
    fn reversed_drag_is_a_rectangle() {
        let mut selection = Selection::default();
        selection.begin(cell(8, 5), false, 10, 8);
        assert!(selection.dragging());
        selection.extend(cell(2, 1), 10, 8);
        let range = selection.range().unwrap();
        assert_eq!(
            range,
            Range {
                first_row: 2,
                last_row: 8,
                first_column: 1,
                last_column: 5,
            }
        );
        assert_eq!(range.row_count(), 7);
        assert_eq!(range.column_count(), 5);
        assert!(range.contains(4, 3));
        assert!(!range.contains(1, 3));
        assert!(!range.contains(4, 6));
        selection.end();
        assert!(!selection.dragging());
        assert_eq!(selection.range(), Some(range));
    }

    #[test]
    fn shift_extension_keeps_the_original_anchor() {
        let mut selection = Selection::default();
        selection.begin(cell(4, 3), false, 12, 9);
        selection.extend(cell(7, 6), 12, 9);
        selection.end();
        selection.begin(cell(1, 0), true, 12, 9);
        assert_eq!(
            selection.range(),
            Some(Range {
                first_row: 1,
                last_row: 4,
                first_column: 0,
                last_column: 3,
            })
        );
        selection.end();
        selection.begin(cell(10, 8), true, 12, 9);
        assert_eq!(
            selection.range(),
            Some(Range {
                first_row: 4,
                last_row: 10,
                first_column: 3,
                last_column: 8,
            })
        );
    }

    #[test]
    fn rows_columns_all_and_clear() {
        let mut selection = Selection::default();
        selection.begin(Target::Row(6), false, 10, 7);
        selection.extend(Target::Row(2), 10, 7);
        assert_eq!(
            selection.range(),
            Some(Range {
                first_row: 2,
                last_row: 6,
                first_column: 0,
                last_column: 6,
            })
        );
        selection.begin(Target::Column(5), false, 10, 7);
        selection.extend(Target::Column(1), 10, 7);
        assert_eq!(
            selection.range(),
            Some(Range {
                first_row: 0,
                last_row: 9,
                first_column: 1,
                last_column: 5,
            })
        );
        selection.all(10, 7);
        assert_eq!(
            selection.range(),
            Some(Range {
                first_row: 0,
                last_row: 9,
                first_column: 0,
                last_column: 6,
            })
        );
        assert!(!selection.dragging());
        selection.clear();
        assert_eq!(selection.range(), None);
        selection.begin(cell(3, 2), true, 10, 7);
        assert_eq!(selection.range().unwrap().row_count(), 1);
        assert_eq!(selection.range().unwrap().column_count(), 1);
    }

    #[test]
    fn targets_are_clamped_and_empty_dimensions_clear() {
        let mut selection = Selection::default();
        selection.begin(cell(u64::MAX, usize::MAX), false, 3, 2);
        assert_eq!(
            selection.range(),
            Some(Range {
                first_row: 2,
                last_row: 2,
                first_column: 1,
                last_column: 1,
            })
        );
        selection.extend(cell(0, 0), 0, 2);
        assert_eq!(selection.range(), None);
        assert!(!selection.dragging());
        selection.all(3, 0);
        assert_eq!(selection.range(), None);
        selection.begin(Target::Column(0), false, 0, 4);
        assert_eq!(selection.range(), None);
    }

    #[test]
    fn tsv_quotes_delimiters_line_breaks_and_quotes() {
        assert_eq!(
            encode_tsv([
                ["plain", "with\ttab", "with\nline"],
                ["with\rcarriage", "say \"hi\"", ""],
            ]),
            "plain\t\"with\ttab\"\t\"with\nline\"\n\"with\rcarriage\"\t\"say \"\"hi\"\"\"\t"
        );
        assert_eq!(encode_tsv([["日本語", "a b"]]), "日本語\ta b");
        assert_eq!(encode_tsv(std::iter::empty::<[&str; 0]>()), "");
    }
}
