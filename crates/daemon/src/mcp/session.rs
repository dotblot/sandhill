//! The tools that read more of the user's live sessions, as Sandhill's
//! panes show them: what ran in the Console (`get_console`), the plots
//! (`get_plots`), a data frame's first rows (`view_table`), a help page
//! (`get_help`) and the installed packages (`list_packages`).

use sandhill_proto::{Package, SessionRun, TableRows};

use super::*;

/// Runs `get_console` returns unless asked, and at most.
const CONSOLE_RUNS: u32 = 10;
const MAX_CONSOLE_RUNS: u32 = 50;
/// Plots `get_plots` returns unless asked; at most `text::MAX_IMAGES`.
const PLOTS: usize = 2;
/// Rows `view_table` returns unless asked, and at most.
const TABLE_ROWS: u32 = 10;
const MAX_TABLE_ROWS: u32 = 100;
/// A table's columns shown, at most, and a cell's characters.
const TABLE_COLUMNS: usize = 30;
const CELL_CHARS: usize = 40;

impl Tools {
    /// The session `session` or `language` names, which must be idle for
    /// code that looks around in it.
    async fn idle_session(
        &self,
        client: &Client,
        session: Option<&str>,
        language: Option<&str>,
        what: &str,
    ) -> Result<SessionInfo> {
        let session = self
            .pick(client, session, language)
            .await?
            .ok_or_else(|| anyhow!("no session is running for that; nothing to {what}"))?;
        if session.pending_runs > 0 {
            bail!(
                "session {} is busy running code; try again once it is idle",
                short_id(&session.id)
            );
        }
        Ok(session)
    }

    pub(super) async fn get_console(
        &self,
        session: Option<&str>,
        language: Option<&str>,
        runs: Option<u64>,
    ) -> Result<Vec<Value>> {
        let client = self.client().await?;
        let session = self
            .pick(&client, session, language)
            .await?
            .ok_or_else(|| anyhow!("no session is running for that; nothing ran"))?;
        let limit = runs.map_or(CONSOLE_RUNS, |n| (n as u32).clamp(1, MAX_CONSOLE_RUNS));
        let listed = client.session_runs(&session.id, Some(limit)).await?;
        let mut rendered = Rendered {
            text: format!(
                "The last {} run{} in {}, oldest first:\n",
                listed.runs.len(),
                if listed.runs.len() == 1 { "" } else { "s" },
                describe_session(&session)
            ),
            ..Default::default()
        };
        if listed.runs.is_empty() {
            rendered.text.push_str("(nothing has run yet)\n");
        }
        for run in &listed.runs {
            rendered.text.push_str(&format!(
                "\n## {}\nRun ID: {}\n",
                self.run_head(run),
                run.run
            ));
            rendered.text.push_str(&fenced(&run.code));
            let before = rendered.text.len();
            for output in &run.outputs {
                rendered.add_output(output);
            }
            if rendered.text.len() == before {
                rendered.text.push_str("(no output)\n");
            }
        }
        let plots = rendered.images.len();
        if plots > 0 {
            rendered.text.push_str(&format!(
                "\n({plots} plot{} not sent here; get_plots shows them)\n",
                if plots == 1 { "" } else { "s" }
            ));
        }
        Ok(vec![text_item(&rendered.finished_text())])
    }

    /// "agent Claude · ok · 1.2 s · 3 min ago", with the cell it came from.
    fn run_head(&self, run: &SessionRun) -> String {
        let source = match (&run.cell, &run.by) {
            (_, Some(by)) => format!("run by {by}"),
            (Some(cell), None) if !cell.cell_id.starts_with("console-") => {
                format!("cell {} of {}", cell.cell_id, self.shown(&cell.file))
            }
            (Some(cell), None) => format!("a line of {}", self.shown(&cell.file)),
            (None, None) => "typed in the Console".into(),
        };
        match (&run.status, run.finished_ms) {
            (Some(status), Some(finished)) => format!(
                "{source} · {status} · {} · {} ago",
                text::duration_ms(finished - run.started_ms),
                ago(now_ms() - finished)
            ),
            _ => format!("{source} · still running"),
        }
    }

    pub(super) async fn get_plots(
        &self,
        session: Option<&str>,
        language: Option<&str>,
        count: Option<u64>,
    ) -> Result<Vec<Value>> {
        let client = self.client().await?;
        let session = self
            .pick(&client, session, language)
            .await?
            .ok_or_else(|| anyhow!("no session is running for that; it has no plots"))?;
        let count = count.map_or(PLOTS, |n| (n as usize).clamp(1, text::MAX_IMAGES));
        let listed = client
            .session_plots(&session.id, Some(count as u32))
            .await?;
        if listed.plots.is_empty() {
            return Ok(vec![text_item(&format!(
                "No plots yet in {}.",
                describe_session(&session)
            ))]);
        }
        let mut rendered = Rendered {
            text: format!(
                "The last {} plot{} of {}, oldest first:\n",
                listed.plots.len(),
                if listed.plots.len() == 1 { "" } else { "s" },
                describe_session(&session)
            ),
            ..Default::default()
        };
        for (n, plot) in listed.plots.iter().enumerate() {
            let from = match (&plot.cell, &plot.by) {
                (_, Some(by)) => format!("drawn by {by}'s code"),
                (Some(cell), None) => {
                    format!("from cell {} of {}", cell.cell_id, self.shown(&cell.file))
                }
                (None, None) => "from the Console".into(),
            };
            rendered.text.push_str(&format!(
                "\n{}. {from}, {} ago:\n{}",
                n + 1,
                ago(now_ms() - plot.ts_ms),
                fenced(&plot.code)
            ));
            let output = json!({"output_type": "display_data", "data": plot.data});
            let mut one = Rendered::default();
            one.add_output(&output);
            rendered.images.extend(one.images);
        }
        let mut items = vec![text_item(&rendered.finished_text())];
        items.extend(images(&client, &rendered).await);
        Ok(items)
    }

    pub(super) async fn view_table(
        &self,
        name: &str,
        session: Option<&str>,
        language: Option<&str>,
        rows: Option<u64>,
        view: Option<&str>,
    ) -> Result<Vec<Value>> {
        if !matches!(view, None | Some("current") | Some("original")) {
            bail!("view must be `current` or `original`");
        }
        let client = self.client().await?;
        let path = table_path(name);
        let focus = if let Some(project) = self.project.as_deref() {
            client.get_focus(project).await?.and_then(|f| f.table)
        } else {
            None
        };
        let focus =
            focus.filter(|f| f.path == path && session.is_none_or(|s| f.session.starts_with(s)));
        let chosen_session = session.or_else(|| focus.as_ref().map(|f| f.session.as_str()));
        let session = self
            .idle_session(&client, chosen_session, language, "show")
            .await?;
        let current = focus
            .as_ref()
            .filter(|f| view != Some("original") && f.session == session.id);
        let limit = rows
            .map_or(TABLE_ROWS, |n| n.min(u64::from(MAX_TABLE_ROWS)) as u32)
            .max(1);
        let sort = current.map_or(&[][..], |f| f.sort.as_slice());
        let filters = current.map_or(&[][..], |f| f.filters.as_slice());
        let search = current.map_or("", |f| f.search.as_str());
        let table = if let Some(focus) = current.filter(|f| !f.hidden_columns.is_empty()) {
            let header = client
                .table_rows(&session.id, &path, 0, Some(0), &[], &[], "")
                .await?;
            let columns: Vec<u32> = (0..header.width.min(u64::from(u32::MAX)) as u32)
                .filter(|c| !focus.hidden_columns.contains(c))
                .take(TABLE_COLUMNS)
                .collect();
            client
                .call(sandhill_proto::Call::TableRows {
                    session: session.id.clone(),
                    path: path.clone(),
                    offset: 0,
                    limit: Some(limit),
                    sort: sort.to_vec(),
                    filters: filters.to_vec(),
                    search: search.into(),
                    columns: Some(columns),
                    column_offset: 0,
                    column_limit: None,
                    cells_only: false,
                })
                .await
        } else {
            client
                .table_rows(&session.id, &path, 0, Some(limit), sort, filters, search)
                .await
        }
        .with_context(|| format!("{name} could not be read as a table"))?;
        let mut rendered = format_table(name, &table);
        if current.is_some() {
            rendered.push_str(&format!("\nUsing the user's current table view (sort, filters, search, hidden columns): filtered {} of {} rows.\n", table.rows, table.total));
        } else {
            rendered.push_str(
                "\nUsing the original table (no view filters, search, sort, or hidden columns).\n",
            );
        }
        Ok(vec![text_item(&text::shorten(&rendered, text::MAX_TEXT))])
    }

    pub(super) async fn get_help(
        &self,
        topic: &str,
        package: Option<&str>,
        session: Option<&str>,
        language: Option<&str>,
    ) -> Result<Vec<Value>> {
        let client = self.client().await?;
        let session = self
            .idle_session(&client, session, language, "look up")
            .await?;
        let page = client
            .help_page(&session.id, topic, package)
            .await
            .with_context(|| format!("no help page for {topic}"))?;
        let mut out = format!("# {}", page.topic);
        if !page.package.is_empty() {
            out.push_str(&format!(" ({}", page.package));
            if !page.version.is_empty() {
                out.push_str(&format!(" {}", page.version));
            }
            out.push(')');
        }
        out.push('\n');
        if !page.title.is_empty() {
            out.push_str(&format!("{}\n", page.title));
        }
        if let Some(signature) = &page.signature {
            out.push_str(&format!("\n{}{signature}\n", page.topic));
        }
        let body = page
            .text
            .clone()
            .or_else(|| page.html.as_deref().map(text::html_to_text))
            .unwrap_or_default();
        if !body.trim().is_empty() {
            out.push('\n');
            out.push_str(body.trim());
            out.push('\n');
        }
        if !page.members.is_empty() {
            let names: Vec<&str> = page.members.iter().map(|m| m.name.as_str()).collect();
            out.push_str(&format!("\nMembers: {}\n", names.join(", ")));
        }
        if !page.alternatives.is_empty() {
            let others: Vec<String> = page
                .alternatives
                .iter()
                .map(|t| format!("{}::{}", t.package, t.topic))
                .collect();
            out.push_str(&format!("\nAlso: {}\n", others.join(", ")));
        }
        Ok(vec![text_item(&text::shorten(&out, text::MAX_TEXT))])
    }

    pub(super) async fn list_packages(
        &self,
        filter: Option<&str>,
        session: Option<&str>,
        language: Option<&str>,
    ) -> Result<Vec<Value>> {
        let client = self.client().await?;
        let session = self
            .idle_session(&client, session, language, "list")
            .await?;
        let listed = client.list_packages(&session.id).await?;
        let wanted = filter.map(str::to_ascii_lowercase);
        let packages: Vec<&Package> = listed
            .packages
            .iter()
            .filter(|p| {
                wanted
                    .as_deref()
                    .is_none_or(|w| p.name.to_ascii_lowercase().contains(w))
            })
            .collect();
        let head = format!(
            "{} package{} installed for {}{}:",
            packages.len(),
            if packages.len() == 1 { "" } else { "s" },
            describe_session(&session),
            filter
                .map(|f| format!(" matching {f:?}"))
                .unwrap_or_default()
        );
        let lines: Vec<String> = packages
            .iter()
            .map(|p| {
                let state = if p.attached {
                    " (attached)"
                } else if p.loaded {
                    " (loaded)"
                } else {
                    ""
                };
                format!("{} {}{state}", p.name, p.version)
            })
            .collect();
        Ok(vec![text_item(&text::shorten(
            &format!("{head}\n{}", lines.join("\n")),
            text::MAX_TEXT,
        ))])
    }
}

/// Code as a fenced block.
fn fenced(code: &str) -> String {
    format!("```\n{}\n```\n", code.trim_end())
}

/// The path to a table: a variable, or a part of one written as R or
/// Python reaches it (`seurat@meta.data`, `adata.obs`, `x$counts`).
fn table_path(name: &str) -> Vec<String> {
    name.split(['$', '@'])
        .flat_map(|part| {
            // `adata.obs` is a part; R's `meta.data` after an `@` is a name.
            if name.contains(['$', '@']) {
                vec![part.to_string()]
            } else {
                part.split('.').map(str::to_string).collect()
            }
        })
        .filter(|p| !p.is_empty())
        .collect()
}

/// A table's first rows as aligned text, with its size and column types.
fn format_table(name: &str, table: &TableRows) -> String {
    let shown_columns = table.columns.len().min(TABLE_COLUMNS);
    let mut out = format!(
        "{name}: {} rows × {} columns{}\n",
        table.total,
        table.width,
        if table.matrix { " (a matrix)" } else { "" }
    );
    let types: Vec<String> = table
        .columns
        .iter()
        .take(shown_columns)
        .map(|c| format!("{} <{}>", c.name, c.kind_detail))
        .collect();
    out.push_str(&format!("Columns: {}\n", types.join(", ")));
    if table.columns.len() > shown_columns || table.width as usize > shown_columns {
        out.push_str(&format!(
            "(only the first {shown_columns} columns are shown)\n"
        ));
    }
    let cell = |v: &Value| -> String {
        let s = match v {
            Value::Null => "NA".to_string(),
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        let s = s.replace(['\n', '\t'], " ");
        if s.chars().count() > CELL_CHARS {
            format!("{}…", s.chars().take(CELL_CHARS - 1).collect::<String>())
        } else {
            s
        }
    };
    let mut grid: Vec<Vec<String>> = Vec::new();
    let mut header = vec![String::new()];
    header.extend(
        table
            .columns
            .iter()
            .take(shown_columns)
            .map(|c| c.name.clone()),
    );
    grid.push(header);
    for (label, values) in table.labels.iter().zip(&table.values) {
        let mut row = vec![label.clone()];
        row.extend(values.iter().take(shown_columns).map(cell));
        grid.push(row);
    }
    let widths: Vec<usize> = (0..grid[0].len())
        .map(|i| {
            grid.iter()
                .filter_map(|r| r.get(i))
                .map(|s| s.chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    out.push('\n');
    for row in &grid {
        let line: Vec<String> = row
            .iter()
            .zip(&widths)
            .map(|(s, w)| format!("{s:w$}"))
            .collect();
        out.push_str(line.join("  ").trim_end());
        out.push('\n');
    }
    if (table.values.len() as u64) < table.rows {
        out.push_str(&format!(
            "(the first {} of {} rows)\n",
            table.values.len(),
            table.rows
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use sandhill_proto::TableColumn;

    #[test]
    fn reaches_parts_of_objects() {
        assert_eq!(table_path("df"), ["df"]);
        assert_eq!(table_path("adata.obs"), ["adata", "obs"]);
        assert_eq!(table_path("seurat@meta.data"), ["seurat", "meta.data"]);
        assert_eq!(table_path("x$counts"), ["x", "counts"]);
    }

    #[test]
    fn shows_a_tables_first_rows_aligned() {
        let table = TableRows {
            session: "s".into(),
            path: vec!["df".into()],
            code: String::new(),
            matrix: false,
            columns: vec![
                TableColumn {
                    name: "gene".into(),
                    kind: "string".into(),
                    kind_detail: "chr".into(),
                },
                TableColumn {
                    name: "count".into(),
                    kind: "number".into(),
                    kind_detail: "int".into(),
                },
            ],
            column_indices: None,
            column_offset: 0,
            width: 2,
            total: 3,
            rows: 3,
            offset: 0,
            labels: vec!["1".into(), "2".into()],
            values: vec![
                vec![json!("TP53"), json!(120)],
                vec![json!("BRCA1"), Value::Null],
            ],
        };
        let text = format_table("df", &table);
        assert!(text.starts_with("df: 3 rows × 2 columns\n"), "{text}");
        assert!(text.contains("gene <chr>, count <int>"), "{text}");
        assert!(text.contains("1  TP53   120"), "{text}");
        assert!(text.contains("2  BRCA1  NA"), "{text}");
        assert!(text.contains("(the first 2 of 3 rows)"), "{text}");
    }
}
