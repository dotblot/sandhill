//! Calls that list and find variables and read, summarize and export tables.

use super::*;

impl Daemon {
    pub(super) async fn list_variables(&self, session: String, path: Vec<String>) -> Result<Value> {
        let printed = self
            .quietly(&session, "list the variables", |language| {
                variables::list_code(language, &path)
            })
            .await?;
        let (list, more) = variables::parse(&printed.stdout);
        Ok(serde_json::to_value(Variables {
            session,
            path,
            variables: list,
            more,
        })?)
    }

    pub(super) async fn find_variables(
        &self,
        session: String,
        query: String,
        root: Vec<String>,
    ) -> Result<Value> {
        let printed = self
            .quietly(&session, "search the variables", |language| {
                crate::variable_search::code(language, &query, &root)
            })
            .await?;
        let (matches, complete) = crate::variable_search::parse(&printed.stdout);
        Ok(serde_json::to_value(sandhill_proto::VariableMatches {
            session,
            matches,
            complete,
        })?)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn table_rows(
        &self,
        session: String,
        path: Vec<String>,
        offset: u64,
        limit: Option<u32>,
        sort: Vec<sandhill_proto::TableSort>,
        filters: Vec<sandhill_proto::TableFilter>,
        search: String,
        columns: Option<Vec<u32>>,
        column_offset: u32,
        column_limit: Option<u32>,
        cells_only: bool,
    ) -> Result<Value> {
        if columns
            .as_ref()
            .is_some_and(|cols| cols.len() > tables::MAX_COLUMNS as usize)
        {
            bail!(
                "at most {} projected columns can be requested",
                tables::MAX_COLUMNS
            );
        }
        let ask = tables::Ask {
            mode: tables::Mode::Rows,
            path: &path,
            sort: &sort,
            filters: &filters,
            search: &search,
            offset,
            limit: limit.unwrap_or(tables::ROWS).min(tables::MAX_ROWS),
            column_offset,
            column_limit: column_limit
                .unwrap_or(tables::COLUMNS)
                .min(tables::MAX_COLUMNS),
            columns: columns.as_deref(),
            headers: !cells_only,
            file: "",
        };
        let printed = self
            .quietly(&session, "read the table", |language| {
                tables::code(language, &ask)
            })
            .await?;
        let rows = tables::parse_rows(&printed.stdout);
        Ok(serde_json::to_value(TableRows {
            session,
            path,
            code: rows.code,
            matrix: rows.matrix,
            columns: rows.columns,
            column_indices: columns,
            column_offset,
            width: rows.width,
            total: rows.total,
            rows: rows.rows,
            offset,
            labels: rows.labels,
            values: rows.values,
        })?)
    }

    pub(super) async fn table_summary(
        &self,
        session: String,
        path: Vec<String>,
        filters: Vec<sandhill_proto::TableFilter>,
        search: String,
        column_offset: u32,
        column_limit: Option<u32>,
    ) -> Result<Value> {
        let ask = tables::Ask {
            mode: tables::Mode::Summary,
            path: &path,
            sort: &[],
            filters: &filters,
            search: &search,
            offset: 0,
            limit: 0,
            column_offset,
            column_limit: column_limit
                .unwrap_or(tables::COLUMNS)
                .min(tables::MAX_COLUMNS),
            columns: None,
            headers: false,
            file: "",
        };
        let printed = self
            .quietly(&session, "summarize the table", |language| {
                tables::code(language, &ask)
            })
            .await?;
        let summary = tables::parse_summary(&printed.stdout);
        Ok(serde_json::to_value(TableSummary {
            session,
            path,
            rows: summary.rows,
            columns: summary.columns,
        })?)
    }

    pub(super) async fn table_export(
        &self,
        session: String,
        path: Vec<String>,
        sort: Vec<sandhill_proto::TableSort>,
        filters: Vec<sandhill_proto::TableFilter>,
        search: String,
        file: String,
    ) -> Result<Value> {
        let resolved = files::resolve(&file)?.to_string_lossy().into_owned();
        let ask = tables::Ask {
            mode: tables::Mode::Export,
            path: &path,
            sort: &sort,
            filters: &filters,
            search: &search,
            offset: 0,
            limit: 0,
            column_offset: 0,
            column_limit: 0,
            columns: None,
            headers: false,
            file: &resolved,
        };
        let printed = self
            .quietly(&session, "export the table", |language| {
                tables::code(language, &ask)
            })
            .await?;
        if tables::parse_written(&printed.stdout).is_none() {
            bail!("could not export the table: nothing was written");
        }
        let meta = std::fs::metadata(&resolved)
            .with_context(|| format!("could not export the table to {file}"))?;
        Ok(serde_json::to_value(FileWritten {
            path: file,
            mtime_ms: files::mtime_ms(&meta),
        })?)
    }
}
