//! The tools for the user's documents: where the user is (`get_focus`), and
//! code written into their document and run there (`add_cell`,
//! `edit_cell`). A document open in a Sandhill window is edited and run by
//! the window, as if the user had typed the code and pressed Run; the tool
//! then follows that run. A document no window has open is edited by the
//! daemon, and the tool runs the cell in the document's session.

use std::cmp::Reverse;

use sandhill_proto::client::ClientError;
use sandhill_proto::{CellEdited, CellOutputs, DocumentsRunIn, Focus};

use super::*;
use crate::cells;

/// How long a window may take to start running the cell it added (it may
/// have to start the document's kernel first).
const WINDOW_RUN_WAIT: Duration = Duration::from_secs(60);
/// Selections longer than this are cut for the agent.
const MAX_SELECTION: usize = 4000;
/// With each prompt, the selection is cut shorter.
const MAX_PROMPT_SELECTION: usize = 1000;
/// A place older than this is not mentioned with a prompt: the user has
/// likely been away from Sandhill.
const STALE_FOCUS_MS: i64 = 30 * 60_000;
/// A script's console keeps this many runs, as `console-0` to `console-99`
/// (as in `app/src/docs/scriptDoc.ts`).
const CONSOLE_SLOTS: u32 = 100;

/// What `add_cell` or `edit_cell` was asked to do.
pub(super) struct Edit<'a> {
    pub file: Option<&'a str>,
    pub code: &'a str,
    pub language: Option<&'a str>,
    /// For add_cell, the cell to go after; for edit_cell, the cell to change.
    pub cell: Option<&'a str>,
    pub run: bool,
    pub timeout: Duration,
}

/// What goes with a message the user sends (see [`Tools::prompt`]).
#[derive(Debug, Default)]
pub struct Prompt {
    /// What the agent is told with it.
    pub context: Option<String>,
    /// What the chat shows went with it: `{file?, cell?, line?, refs?}`,
    /// the file as shown to the agent.
    pub place: Option<Value>,
    /// What the message changes in the terminal's report: the file it
    /// works in (`document`), and the refs it took.
    pub report: Value,
}

impl Tools {
    pub(super) async fn get_focus(&self) -> Result<Vec<Value>> {
        let client = self.client().await?;
        let linked = self.linked_document(&client).await;
        let mut text = match self.focus(&client).await {
            Some(focus) => self.describe_focus(&focus).await,
            None => "Sandhill hasn't said where the user is: no window of this project has been open since it started.".to_string(),
        };
        if let Some(linked) = linked {
            text.push_str(&format!(
                "\nThis conversation last worked in {}; add_cell writes there unless told otherwise.",
                self.shown(&linked)
            ));
        }
        Ok(vec![text_item(&text)])
    }

    /// What goes with each message the user sends (Claude Code's
    /// UserPromptSubmit hook, Codex's turns): a few lines for the agent and
    /// what the chat shows went with it, from the user's choices in the
    /// message box (see [`crate::terminal::choices`]) and where they are,
    /// so "this cell" and "the selection" mean something.
    pub async fn prompt(&self, wait: Duration) -> Prompt {
        let choices = self
            .terminal_dir
            .as_deref()
            .map(crate::terminal::choices::read)
            .unwrap_or_default();
        let focus = tokio::time::timeout(wait, async {
            let client = self.client().await.ok()?;
            self.focus(&client).await
        })
        .await
        .ok()
        .flatten()
        .filter(|f| now_ms() - f.ts_ms <= STALE_FOCUS_MS);
        let front = focus
            .as_ref()
            .and_then(|f| f.file.clone())
            .filter(|f| cells::kind(Path::new(f)).is_some());
        let mut lines = Vec::new();
        let mut place = json!({});
        if let Some(focus) = focus.as_ref().filter(|_| choices.context)
            && let Some(file) = focus.file.as_deref()
        {
            place["file"] = json!(self.shown(file));
            if let Some(cell) = &focus.cell {
                place["cell"] = json!(cell);
            }
            if let Some(line) = focus.line {
                place["line"] = json!(line);
            }
            let mut text = format!("Sandhill: the user is in {}", self.shown(file));
            if let Some(cell) = &focus.cell {
                text.push_str(&format!(", cell {cell}"));
            }
            if let Some(line) = focus.line {
                text.push_str(&format!(", line {line}"));
            }
            text.push('.');
            if let Some(selection) = focus.selection.as_deref().filter(|s| !s.trim().is_empty()) {
                text.push_str(&format!(
                    " They have selected:\n```\n{}\n```",
                    text::shorten(selection, MAX_PROMPT_SELECTION)
                ));
            }
            lines.push(text);
        }
        // The file this message works in: a document the user named with @,
        // else the pinned one, else the one in front, kept while the agent
        // works on it. Conversations are about the whole project, so the
        // message says which document, not the conversation.
        let named = choices
            .refs
            .iter()
            .filter(|r| r.kind == "file" || r.kind == "chunk")
            .filter_map(|r| r.path.as_deref())
            .map(|p| self.path(p))
            .find(|p| cells::kind(Path::new(p)).is_some());
        let document = named
            .or_else(|| choices.pin.as_deref().map(|p| self.path(p)))
            .or(front);
        if let Some(pin) = &choices.pin {
            lines.push(format!(
                "Sandhill: the user pinned this conversation to {}; add_cell writes there.",
                self.shown(&self.path(pin))
            ));
        }
        if let Some(refs) = crate::terminal::choices::refs_text(&choices.refs) {
            lines.push(refs);
            place["refs"] = json!(choices.refs.iter().map(|r| &r.name).collect::<Vec<_>>());
        }
        if !choices.web {
            lines.push(crate::terminal::choices::WEB_OFF_NOTE.to_string());
        }
        let mut report = json!({});
        if let Some(document) = document {
            report["document"] = json!(document);
        }
        if !choices.refs.is_empty() {
            report["refs"] = Value::Null;
        }
        Prompt {
            context: (!lines.is_empty()).then(|| lines.join("\n")),
            place: (place.as_object().is_some_and(|p| !p.is_empty())).then_some(place),
            report,
        }
    }

    pub(super) async fn add_cell(&self, edit: Edit<'_>) -> Result<Vec<Value>> {
        if self
            .terminal_dir
            .as_deref()
            .is_some_and(crate::terminal::code_to_console)
        {
            return self.add_to_console(edit).await;
        }
        let mut client = self.client().await?;
        let focus = self.focus(&client).await;
        let file = self.target(&client, edit.file, focus.as_ref()).await?;
        // Below the user's current cell, when they are in this document.
        let mut after = edit.cell.map(str::to_string).or_else(|| {
            focus
                .as_ref()
                .filter(|f| f.file.as_deref() == Some(file.as_str()))
                .and_then(|f| f.cell.clone())
        });
        let call = |after: Option<String>| Call::AddCell {
            file: file.clone(),
            code: edit.code.into(),
            language: edit.language.map(Into::into),
            after,
            run: edit.run,
            project: self.project.clone(),
        };
        let since = now_ms();
        let edited: CellEdited = match client.call(call(after.clone())).await {
            Ok(edited) => edited,
            // The cell the user was last seen in may be gone.
            Err(ClientError::Remote(message))
                if edit.cell.is_none() && after.is_some() && message.contains("has no cell") =>
            {
                after = None;
                client.call(call(None)).await?
            }
            Err(err) => return Err(err.into()),
        };
        let place = match &after {
            Some(cell) => format!("below cell {cell}"),
            None => "after the last cell".into(),
        };
        let head = format!(
            "Added {} to {} {place}",
            describe_cell(&edited),
            self.shown(&edited.file)
        );
        self.finish(&mut client, &edited, edit, head, since).await
    }

    /// `add_cell` when the user has the agent's code go to the Console:
    /// the code runs in the document's session, where the Console shows it
    /// as the agent's, and the document is left as it is.
    async fn add_to_console(&self, edit: Edit<'_>) -> Result<Vec<Value>> {
        let mut client = self.client().await?;
        let focus = self.focus(&client).await;
        let file = self.target(&client, edit.file, focus.as_ref()).await.ok();
        let language = match edit.language {
            Some(language) => normalize_language(language),
            None => self
                .language_of(&client, file.as_deref(), focus.as_ref())
                .await
                .context("say which language with `language` (r or python)")?,
        };
        let session = match &file {
            Some(file) => {
                let document = CellEdited {
                    file: file.clone(),
                    cell_id: String::new(),
                    language: language.clone(),
                    in_window: false,
                    kernel: None,
                };
                self.session_for(&client, &document).await?
            }
            None => match self.pick(&client, None, Some(&language)).await? {
                Some(session) => session,
                None => self.start_session(&client, &language).await?,
            },
        };
        let instead = file
            .as_deref()
            .map(|f| format!(" instead of adding it to {}", self.shown(f)))
            .unwrap_or_default();
        if !edit.run {
            return Ok(vec![text_item(&format!(
                "The user has your code going to the Console{instead}, so nothing was added and nothing ran. Use run_code to run it there."
            ))]);
        }
        let by = self.agent_name();
        let outcome = run(&mut client, &session, edit.code, None, by, edit.timeout).await?;
        let head = format!(
            "Ran it in the Console, in {}{instead}: the user has your code going to the Console",
            describe_session(&session)
        );
        Ok(report(&client, &head, &outcome).await)
    }

    /// The language code for `file` is in: a script's own, else that of the
    /// cell the user is in, else that of the cell that ran last.
    async fn language_of(
        &self,
        client: &Client,
        file: Option<&str>,
        focus: Option<&Focus>,
    ) -> Option<String> {
        let file = file?;
        if let Some(cells::Kind::Script(own)) = cells::kind(Path::new(file)) {
            return Some(own.into());
        }
        let here = focus
            .filter(|f| f.file.as_deref() == Some(file))
            .and_then(|f| f.cell.clone());
        if let Some(cell) = here {
            let path = file.to_string();
            let code = tokio::task::spawn_blocking(move || cells::cell_code(&path, &cell))
                .await
                .ok()
                .and_then(Result::ok)
                .flatten();
            if let Some((language, _)) = code {
                return Some(normalize_language(&language));
            }
        }
        let outputs = client.file_outputs(file).await.ok()?;
        outputs
            .cells
            .iter()
            .max_by_key(|c| c.started_ms)
            .map(|c| normalize_language(&c.language))
    }

    pub(super) async fn edit_cell(&self, edit: Edit<'_>) -> Result<Vec<Value>> {
        let mut client = self.client().await?;
        let cell = edit.cell.context("`cell` is required")?;
        let focus = self.focus(&client).await;
        let file = self.target(&client, edit.file, focus.as_ref()).await?;
        let since = now_ms();
        let edited: CellEdited = client
            .call(Call::EditCell {
                file,
                cell_id: cell.into(),
                code: edit.code.into(),
                run: edit.run,
                project: self.project.clone(),
            })
            .await?;
        let head = format!(
            "Changed {} of {}",
            describe_cell(&edited),
            self.shown(&edited.file)
        );
        self.finish(&mut client, &edited, edit, head, since).await
    }

    /// Runs the cell, or follows the run the window started, and reports.
    async fn finish(
        &self,
        client: &mut Client,
        edited: &CellEdited,
        edit: Edit<'_>,
        head: String,
        since: i64,
    ) -> Result<Vec<Value>> {
        if !edit.run {
            return Ok(vec![text_item(&format!("{head}. Not run."))]);
        }
        if !edited.in_window {
            let session = self.session_for(client, edited).await?;
            let cell_id = if edited.cell_id.is_empty() {
                // A script's code shows in its console when the file opens.
                next_console_slot(&client.file_outputs(&edited.file).await?)
            } else {
                edited.cell_id.clone()
            };
            let cell = CellRef {
                file: edited.file.clone(),
                cell_id,
            };
            let by = self.agent_name();
            let outcome = run(client, &session, edit.code, Some(cell), by, edit.timeout).await?;
            let head = format!("{head}, and ran it in {}", describe_session(&session));
            return Ok(report(client, &head, &outcome).await);
        }
        // The window runs it, as if the user pressed Run.
        let started = Instant::now();
        let Some((cell, seq)) = wait_for_run(
            client,
            &edited.file,
            &edited.cell_id,
            since,
            WINDOW_RUN_WAIT.min(edit.timeout),
        )
        .await?
        else {
            return Ok(vec![text_item(&format!(
                "{head}. Sandhill's window was asked to run it but hasn't started it yet; get_outputs shows its output once it has run."
            ))]);
        };
        let mut outcome = Outcome::default();
        for output in &cell.outputs {
            outcome.rendered.add_output(output);
        }
        if cell.finished_ms.is_some() {
            outcome.finished = true;
            outcome.status = cell.status.clone();
        } else {
            client.subscribe(&cell.session, Some(seq)).await?;
            let deadline = tokio::time::Instant::from_std(started + edit.timeout);
            follow(client, &cell.run, &mut outcome, deadline).await?;
            let _ = client.unsubscribe(&cell.session).await;
        }
        outcome.elapsed_ms = started.elapsed().as_millis() as i64;
        let session = client
            .list_sessions()
            .await?
            .into_iter()
            .find(|s| s.id == cell.session)
            .map(|s| describe_session(&s))
            .unwrap_or_else(|| format!("session {}", short_id(&cell.session)));
        let head = format!("{head}, and Sandhill ran it in {session}");
        Ok(report(client, &head, &outcome).await)
    }

    async fn focus(&self, client: &Client) -> Option<Focus> {
        client
            .get_focus(self.project.as_deref()?)
            .await
            .ok()
            .flatten()
    }

    /// The document this conversation works in: the one its last message
    /// worked in, else the one the user pinned it to, else the one it was
    /// started about, if any.
    async fn linked_document(&self, client: &Client) -> Option<String> {
        if let Some(dir) = self.terminal_dir.as_deref() {
            let report = crate::terminal::choices::report(dir);
            let chosen = report["document"]
                .as_str()
                .map(str::to_string)
                .or_else(|| crate::terminal::choices::of(&report).pin);
            if let Some(doc) = chosen.filter(|d| !d.is_empty()) {
                return Some(self.path(&doc));
            }
        }
        let terminal = self.terminal.as_deref()?;
        let terminals = client.list_terminals().await.ok()?;
        let info = terminals.into_iter().find(|t| t.id == terminal)?;
        let doc = info.options.documents.into_iter().next()?;
        Some(self.path(&doc))
    }

    /// The document to edit: the one named, else the one this conversation
    /// is about, else the one in front.
    async fn target(
        &self,
        client: &Client,
        file: Option<&str>,
        focus: Option<&Focus>,
    ) -> Result<String> {
        if let Some(file) = file {
            return Ok(self.path(file));
        }
        if let Some(linked) = self.linked_document(client).await {
            return Ok(linked);
        }
        if let Some(front) = focus
            .and_then(|f| f.file.as_deref())
            .filter(|f| cells::kind(Path::new(f)).is_some())
        {
            return Ok(front.to_string());
        }
        bail!(
            "say which document with `file`: no document is linked to this conversation, and the user has none in front"
        )
    }

    async fn describe_focus(&self, focus: &Focus) -> String {
        let Some(file) = &focus.file else {
            return "The user has no document in front.".into();
        };
        let mut text = format!("The user is in {}", self.shown(file));
        if let Some(cell) = &focus.cell {
            text.push_str(&format!(", cell {cell}"));
        }
        if let Some(line) = focus.line {
            text.push_str(&format!(", line {line}"));
        }
        let age = now_ms() - focus.ts_ms;
        if age > 120_000 {
            text.push_str(&format!(" (as of {} ago)", ago(age)));
        }
        text.push('.');
        if let Some(cell) = focus.cell.clone() {
            let path = file.clone();
            let code = tokio::task::spawn_blocking(move || cells::cell_code(&path, &cell))
                .await
                .ok()
                .and_then(Result::ok)
                .flatten();
            if let Some((language, code)) = code {
                text.push_str(&format!(
                    "\nThat cell's code, as saved:\n```{language}\n{}\n```",
                    code.trim_end()
                ));
            }
        }
        if let Some(selection) = focus.selection.as_deref().filter(|s| !s.trim().is_empty()) {
            text.push_str(&format!(
                "\nSelected:\n```\n{}\n```",
                text::shorten(selection, MAX_SELECTION)
            ));
        }
        let others: Vec<String> = focus
            .open
            .iter()
            .filter(|f| *f != file)
            .map(|f| self.shown(f))
            .collect();
        if !others.is_empty() {
            text.push_str(&format!("\nAlso open: {}.", others.join(", ")));
        }
        text
    }

    /// The session a document's cell runs in when no window runs it: the
    /// one that last ran the document's cells, else one of this project in
    /// the document's folder (the folder documents share sessions by), else
    /// a new one, of the kernel the document asks for.
    async fn session_for(&self, client: &Client, edited: &CellEdited) -> Result<SessionInfo> {
        let language = normalize_language(&edited.language);
        let running: Vec<SessionInfo> = client
            .list_sessions()
            .await?
            .into_iter()
            .filter(|s| {
                s.status == SessionStatus::Running && normalize_language(&s.language) == language
            })
            .collect();
        let mut ran: Vec<CellOutputs> = client
            .file_outputs(&edited.file)
            .await?
            .cells
            .into_iter()
            .filter(|c| normalize_language(&c.language) == language)
            .collect();
        ran.sort_by_key(|c| Reverse(c.started_ms));
        for cell in &ran {
            if let Some(session) = running.iter().find(|s| s.id == cell.session) {
                return Ok(session.clone());
            }
        }
        let path = Path::new(&edited.file);
        let setup = self.project_setup(client).await?;
        let in_root = setup
            .as_ref()
            .is_some_and(|s| s.settings.documents == DocumentsRunIn::Project);
        let folder = match (cells::kind(path), &self.project) {
            // Scripts run in the project's folder, as in RStudio, and so do
            // documents when the project says so.
            (Some(cells::Kind::Script(_)), Some(project)) => project.clone(),
            (_, Some(project)) if in_root => project.clone(),
            _ => path
                .parent()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default(),
        };
        // The kernel the project runs the language in wins over the one
        // the document names.
        let chosen = setup.and_then(|s| if language == "r" { s.r } else { s.python });
        let kernel = chosen.as_deref().or(edited.kernel.as_deref());
        let mut shared: Vec<&SessionInfo> = running
            .iter()
            .filter(|s| self.in_project(s) && s.cwd == folder)
            .filter(|s| kernel.is_none_or(|k| s.kernel == k))
            .collect();
        shared.sort_by_key(|s| Reverse(s.last_seq));
        if let Some(session) = shared.first() {
            return Ok((*session).clone());
        }
        self.start_kernel(client, &language, kernel, Some(folder))
            .await
    }
}

/// Waits for a run of `cell_id` that started at `since` or later.
async fn wait_for_run(
    client: &Client,
    file: &str,
    cell_id: &str,
    since: i64,
    wait: Duration,
) -> Result<Option<(CellOutputs, i64)>> {
    let deadline = Instant::now() + wait;
    loop {
        let outputs = client.file_outputs(file).await?;
        if let Some(cell) = outputs
            .cells
            .into_iter()
            .find(|c| c.cell_id == cell_id && c.started_ms >= since)
        {
            return Ok(Some((cell, outputs.seq)));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// The console entry after a script's latest one.
fn next_console_slot(outputs: &FileOutputs) -> String {
    let latest = outputs
        .cells
        .iter()
        .filter_map(|c| {
            let n = c.cell_id.strip_prefix("console-")?.parse::<u32>().ok()?;
            Some((c.started_ms, n))
        })
        .max();
    format!(
        "console-{}",
        latest.map_or(0, |(_, n)| (n + 1) % CONSOLE_SLOTS)
    )
}

/// "cell chunk-3", "a code cell (3f2a9c1e)", or "the code" for a script.
fn describe_cell(edited: &CellEdited) -> String {
    if edited.cell_id.is_empty() || edited.cell_id.starts_with("console-") {
        format!("the {} code", language_name(&edited.language))
    } else {
        format!(
            "{} cell {}",
            language_name(&edited.language),
            edited.cell_id
        )
    }
}

fn language_name(language: &str) -> &str {
    match language {
        "r" => "R",
        "python" => "Python",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(id: &str, started_ms: i64) -> CellOutputs {
        CellOutputs {
            cell_id: id.into(),
            run: "r".into(),
            session: "s".into(),
            language: "r".into(),
            code: String::new(),
            started_ms,
            finished_ms: None,
            status: None,
            execution_count: None,
            outputs: vec![],
        }
    }

    #[test]
    fn script_code_goes_to_the_next_console_entry() {
        let mut outputs = FileOutputs {
            file: "/p/a.R".into(),
            cells: vec![],
            seq: 0,
        };
        assert_eq!(next_console_slot(&outputs), "console-0");
        outputs.cells = vec![
            cell("console-98", 5),
            cell("console-99", 7),
            cell("console-3", 2),
        ];
        assert_eq!(next_console_slot(&outputs), "console-0");
        outputs.cells.push(cell("console-0", 9));
        assert_eq!(next_console_slot(&outputs), "console-1");
    }
}
