use super::*;

impl Explorer {
    pub(super) fn rows_request(&self, block: u64) -> Call {
        Call::TableRows {
            session: self.session.clone(),
            path: self.path.clone(),
            offset: block * BLOCK,
            limit: Some(BLOCK as u32),
            sort: self.sort.clone(),
            filters: self.filters.clone(),
            search: self.search.clone(),
            column_offset: 0,
            column_limit: Some(COLUMNS),
            cells_only: block != 0,
            // The first block keeps complete column metadata for the sidebar.
            columns: (block != 0).then(|| self.visible_columns()),
        }
    }
    pub(super) fn invalidate_rows(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        self.blocks.clear();
        if self.busy {
            self.stale = true;
        } else {
            self.load(0, cx);
            if self.summary.is_empty() {
                self.read_summary(cx);
            }
        }
    }

    pub(super) fn load(&mut self, block: u64, cx: &mut Context<Self>) {
        if self.blocks.contains_key(&block) {
            return;
        }
        self.blocks.insert(block, Block::Loading);
        let generation = self.generation;
        let first = block == 0;
        let rows = self.daemon.call::<TableRows>(self.rows_request(block));
        cx.spawn(async move |this, cx| {
            let rows = rows.await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                match rows {
                    Ok(rows) => {
                        if first && this.took_shape(&rows) {
                            // The kept filters moved to other columns.
                            this.reload(cx);
                            return;
                        }
                        let slots = rows.column_indices.map(|indices| {
                            Rc::new(
                                indices
                                    .into_iter()
                                    .enumerate()
                                    .map(|(slot, column)| (column, slot))
                                    .collect(),
                            )
                        });
                        let cells = rows
                            .labels
                            .into_iter()
                            .zip(rows.values)
                            .map(|(label, cells)| CachedRow {
                                label,
                                cells,
                                slots: slots.clone(),
                            })
                            .collect();
                        this.blocks.insert(block, Block::Ready(cells));
                        if first {
                            this.selection.clear();
                            cx.emit(FocusChanged);
                            cx.emit(ToolbarChanged);
                        }
                    }
                    Err(err) => {
                        let err = format!("{err:#}");
                        // A busy kernel can't answer: scrolling there again
                        // retries, and the end of the run reads it all again.
                        this.blocks.insert(block, Block::Failed);
                        if err.contains(BUSY) {
                            this.stale = true;
                        } else {
                            this.error = Some(err);
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The first rows of a read: its columns, counts and code. A table of
    /// new columns gets new widths, colours and a picked column. Says
    /// whether the sorting and filters kept from last time had to be moved
    /// to other columns, so the rows must be read again.
    fn took_shape(&mut self, rows: &TableRows) -> bool {
        let same = self.shape.as_ref().is_some_and(|s| {
            s.columns.len() == rows.columns.len()
                && s.columns
                    .iter()
                    .zip(&rows.columns)
                    .all(|(a, b)| a.name == b.name && a.kind == b.kind)
        });
        let previous = (!same)
            .then(|| self.shape.as_ref().map(|s| self.view_of(&s.columns)))
            .flatten();
        if !same {
            self.resized.clear();
            // A first guess from character counts, measured properly once
            // the window can lay the first rows out (`fit_column_widths`).
            self.fit_widths = true;
            self.widths = rows
                .columns
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    let cells: Vec<String> = rows
                        .values
                        .iter()
                        .take(50)
                        .filter_map(|row| row.get(i))
                        .map(|v| format_cell(v, &self.language).0)
                        .collect();
                    column_width(c, &cells)
                })
                .collect();
            let longest = rows
                .labels
                .iter()
                .map(|l| l.chars().count())
                .chain([2, rows.total.to_string().len()])
                .max()
                .unwrap_or(2);
            self.label_width = (longest as f32 * 8.4 + 24.).clamp(44., 240.).round();
            self.colors.clear();
            self.overall.clear();
            if self.picked >= rows.columns.len() {
                self.picked = 0;
            }
        }
        self.shape = Some(Shape {
            code: rows.code.clone(),
            matrix: rows.matrix,
            columns: rows.columns.clone(),
            width: rows.width,
            total: rows.total,
            rows: rows.rows,
        });
        // The summary of this same read can arrive before its first rows:
        // its category colours wait until the columns are known.
        if !same && self.summary_generation == self.generation {
            let summaries: Vec<ColumnSummary> = self.summary.values().cloned().collect();
            self.color_values(&summaries);
            if self.filters.is_empty() && self.search.is_empty() {
                self.overall = self.summary.clone();
            }
        }
        let Some(saved) = self.saved.take().or(previous) else {
            return false;
        };
        let (sort, filters) = saved.on(&rows.columns);
        self.hidden = saved.hidden_on(&rows.columns);
        self.pinned = saved.pinned_on(&rows.columns);
        for (column, width) in saved.widths_on(&rows.columns) {
            if let Some(slot) = self.widths.get_mut(column as usize) {
                *slot = width.clamp(MIN_COLUMN_WIDTH, MAX_COLUMN_WIDTH);
                self.resized.push(column);
            }
        }
        if sort == self.sort && filters == self.filters {
            return false;
        }
        self.sort = sort;
        self.filters = filters;
        self.save_view();
        true
    }
    pub(super) fn row(&mut self, ix: usize, cx: &mut Context<Self>) -> Option<CachedRow> {
        let block = ix as u64 / BLOCK;
        match self.blocks.get(&block) {
            Some(Block::Ready(rows)) => rows.get(ix % BLOCK as usize).cloned(),
            Some(Block::Loading) => None,
            Some(Block::Failed) => {
                if !self.busy {
                    self.blocks.remove(&block);
                }
                None
            }
            None => {
                if !self.busy {
                    self.load(block, cx);
                }
                None
            }
        }
    }
}
