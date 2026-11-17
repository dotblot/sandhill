//! The explorer's copying and file export.

use super::*;

impl Explorer {
    // Copy and export.

    pub(super) fn copy_selection(&mut self, cx: &mut Context<Self>) {
        let Some(range) = self.selection.range() else {
            return;
        };
        if range
            .row_count()
            .saturating_mul(range.column_count() as u64)
            > 1_000_000
        {
            self.note = Some((
                "Copy is limited to 1,000,000 cells. Select a smaller range.".into(),
                true,
            ));
            cx.notify();
            return;
        }
        let columns = self.visible_columns()[range.first_column..=range.last_column].to_vec();
        let mut values: Vec<Option<Vec<String>>> = (range.first_row..=range.last_row)
            .map(|row| match self.blocks.get(&(row / BLOCK)) {
                Some(Block::Ready(rows)) => rows.get((row % BLOCK) as usize).and_then(|row| {
                    columns
                        .iter()
                        .map(|column| {
                            let slot =
                                row.slots.as_ref().map_or(Some(*column as usize), |slots| {
                                    slots.get(column).copied()
                                })?;
                            row.cells.get(slot).map(clipboard_value)
                        })
                        .collect()
                }),
                _ => None,
            })
            .collect();
        let requests: Vec<_> = (range.first_row / BLOCK..=range.last_row / BLOCK)
            .filter(|block| {
                let start = (block * BLOCK).max(range.first_row);
                let end = ((block + 1) * BLOCK - 1).min(range.last_row);
                (start..=end).any(|row| values[(row - range.first_row) as usize].is_none())
            })
            .map(|block| {
                let mut request = self.rows_request(block);
                if let Call::TableRows {
                    columns: projection,
                    cells_only,
                    ..
                } = &mut request
                {
                    *projection = Some(columns.clone());
                    *cells_only = true;
                }
                request
            })
            .collect();
        let daemon = self.daemon.clone();
        let generation = self.generation;
        self.note = Some(("Copying selected values…".into(), false));
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result: Result<String, String> = async {
                for request in requests {
                    let offset = match &request {
                        Call::TableRows { offset, .. } => *offset,
                        _ => unreachable!(),
                    };
                    let rows = daemon
                        .call::<TableRows>(request)
                        .await
                        .map_err(|e| format!("{e:#}"))?;
                    let indices = rows.column_indices.as_deref().unwrap_or(&columns);
                    for (i, cells) in rows.values.iter().enumerate() {
                        let row = offset + i as u64;
                        if row < range.first_row || row > range.last_row {
                            continue;
                        }
                        values[(row - range.first_row) as usize] = columns
                            .iter()
                            .map(|column| {
                                let slot = indices.iter().position(|i| i == column)?;
                                cells.get(slot).map(clipboard_value)
                            })
                            .collect();
                    }
                }
                let values: Option<Vec<Vec<String>>> = values.into_iter().collect();
                values.map(encode_tsv).ok_or_else(|| {
                    "The table changed while copying. Refresh and select the range again.".into()
                })
            }
            .await;
            let _ = this.update(cx, |this, cx| {
                this.note = Some(if generation != this.generation {
                    (
                        "The table changed while copying. Select the range again.".into(),
                        true,
                    )
                } else {
                    match result {
                        Ok(text) => {
                            cx.write_to_clipboard(ClipboardItem::new_string(text));
                            ("Copied selected values.".into(), false)
                        }
                        Err(error) => (format!("Could not copy selected values: {error}"), true),
                    }
                });
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn copy_code(&mut self, cx: &mut Context<Self>) {
        let Some(shape) = &self.shape else { return };
        let hidden: Vec<u32> = self.hidden.iter().copied().collect();
        let code = table_code(
            &self.language,
            &shape.code,
            shape.matrix,
            &shape.columns,
            TableCodeView {
                sort: &self.sort,
                filters: &self.filters,
                hidden: &hidden,
                search: &self.search,
            },
        );
        cx.write_to_clipboard(ClipboardItem::new_string(code));
        self.note = Some((
            format!("Copied {} code for this view.", self.language_name()),
            false,
        ));
        cx.notify();
    }

    /// Copies code that applies just the filters, in the session's language.
    pub(super) fn copy_filter_code(&mut self, cx: &mut Context<Self>) {
        let Some(shape) = &self.shape else { return };
        if self.filters.is_empty() {
            return;
        }
        let code = table_code(
            &self.language,
            &shape.code,
            shape.matrix,
            &shape.columns,
            TableCodeView {
                sort: &[],
                filters: &self.filters,
                hidden: &[],
                search: "",
            },
        );
        cx.write_to_clipboard(ClipboardItem::new_string(code));
        self.note = Some((
            format!("Copied {} code for the filters.", self.language_name()),
            false,
        ));
        cx.notify();
    }

    pub(super) fn language_name(&self) -> &'static str {
        if self.language.eq_ignore_ascii_case("r") {
            "R"
        } else {
            "Python"
        }
    }

    pub(super) fn start_export(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = crate::prefs::export_folder(self.project.as_deref().unwrap_or(""))
            .filter(|f| !f.is_empty())
            .map_or_else(
                || export_name(&self.path),
                |f| format!("{f}/{}", export_name(&self.path)),
            );
        self.export_input.update(cx, |i, cx| {
            i.set_value(name, window, cx);
            i.focus(window, cx);
        });
        self.export = Some(Export::Naming);
        cx.notify();
    }

    pub(super) fn export_file(&self, typed: &str) -> String {
        match &self.project {
            Some(project) if !typed.starts_with('/') => {
                format!("{}/{typed}", project.trim_end_matches('/'))
            }
            _ => typed.to_string(),
        }
    }

    /// The name was typed: asks before replacing a file that's there.
    pub(super) fn export_named(&mut self, cx: &mut Context<Self>) {
        let typed = self.export_input.read(cx).value().trim().to_string();
        if typed.is_empty() || !matches!(self.export, Some(Export::Naming)) {
            return;
        }
        let file = self.export_file(&typed);
        self.export = Some(Export::Writing);
        let stat = self
            .daemon
            .call::<Vec<FileStat>>(Call::StatFiles { paths: vec![file] });
        cx.spawn(async move |this, cx| {
            let stat = stat.await;
            this.update(cx, |this, cx| {
                let there = stat
                    .ok()
                    .and_then(|s| s.into_iter().next())
                    .is_some_and(|s| s.mtime_ms.is_some());
                if there {
                    this.export = Some(Export::Replace(typed));
                    cx.notify();
                } else {
                    this.write_export(typed, cx);
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub(super) fn write_export(&mut self, typed: String, cx: &mut Context<Self>) {
        let Some(shape) = &self.shape else { return };
        let rows = shape.rows;
        let file = self.export_file(&typed);
        self.export = Some(Export::Writing);
        let write = self.daemon.call::<Value>(Call::TableExport {
            session: self.session.clone(),
            path: self.path.clone(),
            sort: self.sort.clone(),
            filters: self.filters.clone(),
            search: self.search.clone(),
            file,
        });
        let project = self.project.clone().unwrap_or_default();
        cx.spawn(async move |this, cx| {
            let done = write.await;
            this.update(cx, |this, cx| {
                this.export = None;
                this.note = Some(match done {
                    Ok(_) => {
                        if !typed.starts_with('/') {
                            let folder = typed.rsplit_once('/').map_or("", |(f, _)| f);
                            crate::prefs::save_export_folder(&project, folder);
                        }
                        (
                            format!("Exported {} rows to {typed}.", format_number(rows as f64)),
                            false,
                        )
                    }
                    Err(err) if format!("{err:#}").contains(BUSY) => (
                        format!(
                            "{} is running code; export once it finishes.",
                            this.language_name()
                        ),
                        true,
                    ),
                    Err(err) => (format!("Could not export the table: {err:#}"), true),
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Asks for the export's file name, or whether to replace a file.
    pub(super) fn render_export(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = *Palette::get(cx);
        let export = self.export.as_ref()?;
        let rows = self.shape.as_ref().map_or(0, |s| s.rows);
        let bar = h_flex()
            .flex_none()
            .gap(px(8.))
            .px(px(10.))
            .py(px(6.))
            .bg(p.accent_soft)
            .border_b_1()
            .border_color(p.line);
        let button = |id: &'static str, label: &'static str, primary: bool| {
            let mut b = gpui_kit::component::button::Button::new(id)
                .small()
                .label(label);
            if primary {
                b = gpui_kit::component::button::ButtonVariants::primary(b);
            }
            b
        };
        Some(match export {
            Export::Naming | Export::Writing => bar
                .child(div().flex_none().child("Export the table as"))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(Input::new(&self.export_input).small()),
                )
                .child(div().flex_none().text_color(p.muted).child(format!(
                    "{} rows, filtered and sorted as shown",
                    format_number(rows as f64)
                )))
                .child(
                    button("de-export-go", "Export", true)
                        .on_click(cx.listener(|this, _, _, cx| this.export_named(cx))),
                )
                .child(
                    button("de-export-cancel", "Cancel", false).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.export = None;
                            cx.notify();
                        },
                    )),
                )
                .into_any_element(),
            Export::Replace(name) => {
                let name = name.clone();
                bar.child(div().flex_1().child(format!(
                    "Replace {name}? A file with this name is already there."
                )))
                .child(button("de-replace", "Replace", true).on_click(
                    cx.listener(move |this, _, _, cx| this.write_export(name.clone(), cx)),
                ))
                .child(
                    button("de-replace-cancel", "Cancel", false).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.export = None;
                            cx.notify();
                        },
                    )),
                )
                .into_any_element()
            }
        })
    }
}

fn clipboard_value(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        _ => value.to_string(),
    }
}
