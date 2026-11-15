use super::*;

/// Where the column called `name`, once at `was`, is now.
fn column_now(columns: &[TableColumn], name: &str, was: u32) -> Option<u32> {
    if columns.get(was as usize).is_some_and(|c| c.name == name) {
        return Some(was);
    }
    columns
        .iter()
        .position(|c| c.name == name)
        .map(|i| i as u32)
}

impl SavedView {
    pub(super) fn of(
        sort: &[TableSort],
        filters: &[TableFilter],
        columns: &[TableColumn],
        hidden: &[u32],
    ) -> SavedView {
        let name = |column: u32| {
            columns
                .get(column as usize)
                .map(|c| c.name.clone())
                .unwrap_or_default()
        };
        SavedView {
            sort: sort
                .iter()
                .map(|k| Named {
                    name: name(k.column),
                    key: k.clone(),
                })
                .collect(),
            filters: filters
                .iter()
                .map(|f| Named {
                    name: name(f.column),
                    key: f.clone(),
                })
                .collect(),
            hidden: hidden
                .iter()
                .map(|&column| HiddenColumn {
                    name: name(column),
                    column,
                })
                .collect(),
            pinned: Vec::new(),
            widths: Vec::new(),
        }
    }

    /// With the pinned columns, in their order, and the widths given.
    pub(super) fn with_layout(
        mut self,
        columns: &[TableColumn],
        pinned: &[u32],
        widths: &[(u32, f32)],
    ) -> SavedView {
        let name = |column: u32| columns.get(column as usize).map(|c| c.name.clone());
        self.pinned = pinned
            .iter()
            .filter_map(|&column| {
                Some(HiddenColumn {
                    name: name(column)?,
                    column,
                })
            })
            .collect();
        self.widths = widths
            .iter()
            .filter_map(|&(column, width)| {
                Some(ColumnWidth {
                    name: name(column)?,
                    column,
                    width,
                })
            })
            .collect();
        self
    }

    pub(super) fn is_empty(&self) -> bool {
        self.sort.is_empty()
            && self.filters.is_empty()
            && self.hidden.is_empty()
            && self.pinned.is_empty()
            && self.widths.is_empty()
    }

    /// The sorting and filters on the table's columns now: each moved to
    /// where its column is, and dropped if its column is gone.
    pub(super) fn on(&self, columns: &[TableColumn]) -> (Vec<TableSort>, Vec<TableFilter>) {
        let place = |name: &str, was: u32| {
            if columns.get(was as usize).is_some_and(|c| c.name == name) {
                return Some(was);
            }
            columns
                .iter()
                .position(|c| c.name == name)
                .map(|i| i as u32)
        };
        let sort = self
            .sort
            .iter()
            .filter_map(|k| {
                place(&k.name, k.key.column).map(|column| TableSort {
                    column,
                    ..k.key.clone()
                })
            })
            .collect();
        let filters = self
            .filters
            .iter()
            .filter_map(|f| {
                place(&f.name, f.key.column).map(|column| TableFilter {
                    column,
                    ..f.key.clone()
                })
            })
            .collect();
        (sort, filters)
    }

    pub(super) fn hidden_on(&self, columns: &[TableColumn]) -> Vec<u32> {
        self.hidden
            .iter()
            .filter_map(|h| column_now(columns, &h.name, h.column))
            .collect()
    }

    pub(super) fn pinned_on(&self, columns: &[TableColumn]) -> Vec<u32> {
        self.pinned
            .iter()
            .filter_map(|p| column_now(columns, &p.name, p.column))
            .collect()
    }

    pub(super) fn widths_on(&self, columns: &[TableColumn]) -> Vec<(u32, f32)> {
        self.widths
            .iter()
            .filter_map(|w| Some((column_now(columns, &w.name, w.column)?, w.width)))
            .collect()
    }
}

#[cfg(test)]
mod view_tests {
    use super::SavedView;
    use sandhill_proto::{TableColumn, TableFilter, TableSort};

    fn columns(names: &[&str]) -> Vec<TableColumn> {
        names
            .iter()
            .map(|n| {
                serde_json::from_value(
                    serde_json::json!({"name": n, "kind": "number", "type": "dbl"}),
                )
                .unwrap()
            })
            .collect()
    }

    #[test]
    fn kept_views_follow_their_columns() {
        let sort = vec![TableSort {
            column: 1,
            descending: true,
        }];
        let filters = vec![TableFilter {
            column: 2,
            op: ">".into(),
            values: vec!["200".into()],
        }];
        let saved = SavedView::of(
            &sort,
            &filters,
            &columns(&["cell", "n_genes", "pct_mt"]),
            &[1],
        );
        let json = serde_json::to_value(&saved).unwrap();
        assert_eq!(json["sort"][0]["name"], "n_genes");
        assert_eq!(json["filters"][0]["op"], ">");
        let back: SavedView = serde_json::from_value(json).unwrap();
        assert_eq!(back, saved);
        assert_eq!(
            back.hidden_on(&columns(&["batch", "cell", "n_genes"])),
            vec![2]
        );
        // Same table: as it was.
        assert_eq!(
            back.on(&columns(&["cell", "n_genes", "pct_mt"])),
            (sort.clone(), filters.clone())
        );
        // A column added at the front: they move along; one gone: dropped.
        let (s, f) = back.on(&columns(&["batch", "cell", "n_genes"]));
        assert_eq!(s[0].column, 2);
        assert!(f.is_empty());
    }

    #[test]
    fn old_saved_views_show_every_column() {
        let saved: SavedView =
            serde_json::from_value(serde_json::json!({"sort": [], "filters": []})).unwrap();
        assert!(saved.hidden_on(&columns(&["a", "b"])).is_empty());
    }

    #[test]
    fn kept_pins_and_widths_follow_their_columns() {
        let was = columns(&["cell", "n_genes", "pct_mt"]);
        let saved =
            SavedView::of(&[], &[], &was, &[]).with_layout(&was, &[2, 0], &[(1, 220.), (2, 90.)]);
        assert!(!saved.is_empty());
        let back: SavedView =
            serde_json::from_value(serde_json::to_value(&saved).unwrap()).unwrap();
        // Same table: the pins in the order given, the widths as dragged.
        assert_eq!(back.pinned_on(&was), vec![2, 0]);
        assert_eq!(back.widths_on(&was), vec![(1, 220.), (2, 90.)]);
        // A column added at the front and one gone: moved along, dropped.
        let now = columns(&["batch", "cell", "n_genes"]);
        assert_eq!(back.pinned_on(&now), vec![1]);
        assert_eq!(back.widths_on(&now), vec![(2, 220.)]);
        // A view saved before pins and widths were kept has none.
        let old: SavedView =
            serde_json::from_value(serde_json::json!({"sort": [], "filters": []})).unwrap();
        assert!(old.pinned_on(&was).is_empty() && old.widths_on(&was).is_empty());
        assert!(old.is_empty());
    }
}
