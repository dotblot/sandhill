//! Calls that read and write notebooks and files, add, edit and delete cells, and suggest cells.

use super::*;

impl Daemon {
    pub(super) async fn suggest_cell(
        &self,
        agent: Option<String>,
        cwd: String,
        language: String,
        cells: Vec<String>,
        output: String,
    ) -> Result<Value> {
        let (kind, program) = crate::ghost::choose(agent.as_deref())?;
        let prompt = crate::ghost::prompt(&language, &cells, &output);
        let reply = crate::ghost::ask(kind, &program, Path::new(&cwd), &prompt).await?;
        Ok(serde_json::to_value(sandhill_proto::CellSuggestion {
            agent: kind.into(),
            code: crate::ghost::parse(&reply),
        })?)
    }

    pub(super) async fn read_notebook(&self, path: String) -> Result<Value> {
        let journal = self.journal.clone();
        let file = blocking(move || {
            let file = notebook::read_notebook(&path, journal.blobs());
            memory::release_free_memory();
            file
        })
        .await?;
        Ok(serde_json::to_value(file)?)
    }

    pub(super) async fn write_notebook(
        &self,
        path: String,
        text: String,
        expected_mtime_ms: Option<i64>,
    ) -> Result<Value> {
        let journal = self.journal.clone();
        let written = blocking(move || {
            let written =
                notebook::write_notebook(&path, &text, expected_mtime_ms, journal.blobs());
            drop(text);
            memory::release_free_memory();
            written
        })
        .await?;
        Ok(serde_json::to_value(written)?)
    }

    pub(super) async fn write_file(
        &self,
        path: String,
        text: String,
        expected_mtime_ms: Option<i64>,
        base64: bool,
        create: bool,
    ) -> Result<Value> {
        let bytes = if base64 {
            sandhill_proto::decode_base64(&text).context("the file's bytes are not valid base64")?
        } else {
            text.into_bytes()
        };
        Ok(serde_json::to_value(
            blocking(move || {
                if create {
                    fileops::create_file(&path, &bytes)
                } else {
                    files::write_bytes(&path, &bytes, expected_mtime_ms)
                }
            })
            .await?,
        )?)
    }

    pub(super) async fn file_outputs(&self, file: String) -> Result<Value> {
        let journal = self.journal.clone();
        Ok(serde_json::to_value(
            blocking(move || {
                let file = files::resolve(&file)?.to_string_lossy().into_owned();
                Ok(outputs::file_outputs(&journal, &file)?)
            })
            .await?,
        )?)
    }

    pub(super) async fn clear_cell_outputs(
        &self,
        file: String,
        cell_ids: Vec<String>,
    ) -> Result<Value> {
        let journal = self.journal.clone();
        Ok(serde_json::to_value(
            blocking(move || {
                let file = files::resolve(&file)?.to_string_lossy().into_owned();
                Ok(journal.clear_cell_outputs(&file, &cell_ids)?)
            })
            .await?,
        )?)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn add_cell(
        &self,
        file: String,
        code: String,
        language: Option<String>,
        after: Option<String>,
        run: bool,
        project: Option<String>,
        by: Option<String>,
    ) -> Result<Value> {
        let path = windows::normalize(&files::resolve(&file)?);
        let file = path.to_string_lossy().into_owned();
        let mut params = json!({"file": file, "code": code, "language": language, "after": after, "run": run, "by": by});
        // Left out, not null: no `after` means after the last cell.
        if let Some(map) = params.as_object_mut() {
            map.retain(|_, v| !v.is_null());
        }
        let edited = match self.ask_window(&path, project, "add_cell", params).await? {
            Some(edited) => edited,
            None => {
                let edited = blocking({
                    let file = file.clone();
                    move || cells::add_cell(&file, &code, language.as_deref(), after.as_deref())
                })
                .await?;
                on_disk(file, edited)
            }
        };
        self.windows.moved_to(&path, &edited.cell_id);
        Ok(serde_json::to_value(edited)?)
    }

    pub(super) async fn edit_cell(
        &self,
        file: String,
        cell_id: String,
        code: String,
        run: bool,
        project: Option<String>,
        by: Option<String>,
    ) -> Result<Value> {
        let path = windows::normalize(&files::resolve(&file)?);
        let file = path.to_string_lossy().into_owned();
        let params = json!({"file": file, "cell_id": cell_id, "code": code, "run": run, "by": by});
        let edited = match self.ask_window(&path, project, "edit_cell", params).await? {
            Some(edited) => edited,
            None => {
                let edited = blocking({
                    let file = file.clone();
                    move || cells::edit_cell(&file, &cell_id, &code)
                })
                .await?;
                on_disk(file, edited)
            }
        };
        Ok(serde_json::to_value(edited)?)
    }

    pub(super) async fn delete_cell(
        &self,
        file: String,
        cell_id: String,
        project: Option<String>,
        by: Option<String>,
    ) -> Result<Value> {
        let path = windows::normalize(&files::resolve(&file)?);
        let file = path.to_string_lossy().into_owned();
        let params = json!({"file": file, "cell_id": cell_id, "by": by});
        let edited = match self
            .ask_window(&path, project, "delete_cell", params)
            .await?
        {
            Some(edited) => edited,
            None => {
                let edited = blocking({
                    let file = file.clone();
                    move || cells::delete_cell(&file, &cell_id)
                })
                .await?;
                on_disk(file, edited)
            }
        };
        Ok(serde_json::to_value(edited)?)
    }
}
