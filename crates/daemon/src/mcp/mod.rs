//! `sandhilld mcp`: the tools Sandhill gives coding agents, as an MCP
//! server over stdio. Claude Code and Codex start it themselves (Sandhill
//! passes it on their command line), and it talks to the daemon over its
//! socket like any other client, so it works through daemon restarts.
//!
//! The tools reach the user's live sessions, which is what plain file
//! access can't: `list_sessions`, `list_variables`, `run_code` and
//! `get_outputs`; and the user's documents as they work on them:
//! `get_focus`, `add_cell` and `edit_cell`.

mod documents;
pub mod text;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use sandhill_proto::client::Client;
use sandhill_proto::{
    BLOB_KEY, Call, CellRef, EVENT_RUN_FINISHED, FileOutputs, KernelspecInfo, RunFinished,
    SessionInfo, SessionStatus,
};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

use crate::journal::now_ms;
use text::Rendered;

/// MCP versions this server speaks, newest first.
const PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_TIMEOUT: Duration = Duration::from_secs(3600);
/// Plots larger than this (base64) are mentioned but not sent.
const MAX_IMAGE: usize = 2 << 20;

const INSTRUCTIONS: &str = "Sandhill is the IDE the user runs you in. These tools reach the \
user's live R and Python sessions (Jupyter kernels) for this project: the objects they have \
loaded and the outputs of their notebooks. Use list_variables and get_outputs to look before \
recomputing anything. Code the user asks for goes into their document with add_cell, below the \
cell they are in, and runs there, so they see the code and its output together; fix a cell \
that failed with edit_cell rather than adding another. Use run_code only for quick looks they \
don't need to keep. get_focus tells you where the user is (\"this cell\", the selection).";

pub struct Tools {
    socket: PathBuf,
    /// The project folder the agent works on; sessions of other projects
    /// are left alone unless named.
    project: Option<String>,
    /// The Sandhill terminal the agent runs in, whose session names the
    /// documents it is about.
    terminal: Option<String>,
    /// That terminal's folder, whose report says whether Sandhill's Ask
    /// mode is on (see `terminal::guard`).
    terminal_dir: Option<PathBuf>,
}

/// Serves MCP on stdin and stdout until stdin closes.
pub async fn serve(socket: PathBuf, project: Option<String>) -> Result<()> {
    // The agent passes on its environment, which names its terminal.
    let dir = std::env::var_os("SANDHILL_TERMINAL").map(PathBuf::from);
    let tools = Arc::new(Tools::new(socket, project).in_terminal(dir));
    let (out, mut out_rx) = mpsc::unbounded_channel::<Value>();
    let writer = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        while let Some(message) = out_rx.recv().await {
            let mut line = serde_json::to_vec(&message).unwrap_or_default();
            line.push(b'\n');
            if stdout.write_all(&line).await.is_err() || stdout.flush().await.is_err() {
                return;
            }
        }
    });
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            let _ = out.send(error_reply(Value::Null, -32700, "parse error"));
            continue;
        };
        let Some(method) = message["method"].as_str().map(str::to_string) else {
            continue; // a response to something we never ask
        };
        let Some(id) = message.get("id").cloned() else {
            continue; // notifications need no answer
        };
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        let tools = tools.clone();
        let out = out.clone();
        tokio::spawn(async move {
            let reply = match tools.handle(&method, params).await {
                Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
                Err(err) => error_reply(id, -32601, &format!("{err:#}")),
            };
            let _ = out.send(reply);
        });
    }
    drop(out);
    let _ = writer.await;
    Ok(())
}

fn error_reply(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

impl Tools {
    pub fn new(socket: PathBuf, project: Option<String>) -> Self {
        Tools {
            socket,
            project,
            terminal: None,
            terminal_dir: None,
        }
    }

    /// The tools of an agent in Sandhill terminal `terminal`.
    pub fn with_terminal(mut self, terminal: Option<String>) -> Self {
        self.terminal = terminal;
        self
    }

    /// The tools of an agent in the Sandhill terminal in folder `dir`
    /// (`SANDHILL_TERMINAL`), whose report holds the user's choices for it.
    pub fn in_terminal(mut self, dir: Option<PathBuf>) -> Self {
        self.terminal = dir
            .as_deref()
            .and_then(|dir| Some(dir.file_name()?.to_string_lossy().into_owned()));
        self.terminal_dir = dir;
        self
    }

    async fn handle(&self, method: &str, params: Value) -> Result<Value> {
        Ok(match method {
            "initialize" => {
                let asked = params["protocolVersion"].as_str().unwrap_or("");
                let version = PROTOCOL_VERSIONS
                    .iter()
                    .find(|v| **v == asked)
                    .unwrap_or(&PROTOCOL_VERSIONS[0]);
                json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {"listChanged": false}},
                    "serverInfo": {"name": "sandhill", "version": env!("CARGO_PKG_VERSION")},
                    "instructions": INSTRUCTIONS,
                })
            }
            "ping" => json!({}),
            "tools/list" => json!({"tools": tool_list()}),
            "tools/call" => {
                let name = params["name"].as_str().unwrap_or("");
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                let result = if self.read_only_now() && !read_only_tools().iter().any(|t| t == name)
                {
                    Err(anyhow::anyhow!(crate::terminal::guard::BLOCKED))
                } else {
                    self.call(name, &args).await
                };
                match result {
                    Ok(content) => json!({"content": content, "isError": false}),
                    Err(err) => json!({
                        "content": [{"type": "text", "text": format!("{err:#}")}],
                        "isError": true,
                    }),
                }
            }
            _ => bail!("method not found: {method}"),
        })
    }

    /// Whether the user has the agent in Sandhill's read-only Ask mode.
    fn read_only_now(&self) -> bool {
        self.terminal_dir
            .as_deref()
            .and_then(crate::terminal::guard::current)
            .as_deref()
            == Some(crate::terminal::guard::ASK)
    }

    /// Runs one tool. Returns MCP content items.
    pub async fn call(&self, name: &str, args: &Value) -> Result<Vec<Value>> {
        let text = |s: &str| args[s].as_str().map(str::trim).filter(|s| !s.is_empty());
        match name {
            "list_sessions" => self.list_sessions().await,
            "list_variables" => self.list_variables(text("session"), text("language")).await,
            "run_code" => {
                let code = text("code").context("`code` is required")?;
                self.run_code(code, text("session"), text("language"), timeout(args))
                    .await
            }
            "get_outputs" => {
                let file = text("file").context("`file` is required")?;
                self.get_outputs(file, text("cell")).await
            }
            "get_focus" => self.get_focus().await,
            "add_cell" | "edit_cell" => {
                let code = args["code"]
                    .as_str()
                    .filter(|c| !c.trim().is_empty())
                    .context("`code` is required")?;
                let edit = documents::Edit {
                    file: text("file"),
                    code,
                    language: text("language"),
                    cell: text(if name == "add_cell" { "after" } else { "cell" }),
                    run: args["run"].as_bool().unwrap_or(true),
                    timeout: timeout(args),
                };
                if name == "add_cell" {
                    self.add_cell(edit).await
                } else {
                    self.edit_cell(edit).await
                }
            }
            _ => bail!("no tool named {name}"),
        }
    }

    async fn client(&self) -> Result<Client> {
        Client::connect_unix(&self.socket).await.with_context(|| {
            format!(
                "cannot reach sandhilld at {} (is Sandhill running?)",
                self.socket.display()
            )
        })
    }

    fn in_project(&self, session: &SessionInfo) -> bool {
        match &self.project {
            None => true,
            Some(project) => match &session.project {
                Some(p) => p == project,
                None => Path::new(&session.cwd).starts_with(project),
            },
        }
    }

    /// Running sessions of this project, most recently used first.
    async fn sessions(&self, client: &Client) -> Result<Vec<SessionInfo>> {
        let mut list: Vec<SessionInfo> = client
            .list_sessions()
            .await?
            .into_iter()
            .filter(|s| s.status == SessionStatus::Running && self.in_project(s))
            .collect();
        list.sort_by_key(|s| std::cmp::Reverse(s.last_seq));
        Ok(list)
    }

    /// The session `session` names (an id or its start), or else the
    /// project's latest session for `language`, if any.
    async fn pick(
        &self,
        client: &Client,
        session: Option<&str>,
        language: Option<&str>,
    ) -> Result<Option<SessionInfo>> {
        if let Some(id) = session {
            let all: Vec<SessionInfo> = client
                .list_sessions()
                .await?
                .into_iter()
                .filter(|s| s.status == SessionStatus::Running && s.id.starts_with(id))
                .collect();
            return match all.len() {
                1 => Ok(all.into_iter().next()),
                0 => bail!("no running session {id}; list_sessions shows the running ones"),
                _ => bail!("{id} could be several sessions; give more of its id"),
            };
        }
        let mine = self.sessions(client).await?;
        if let Some(language) = language {
            let language = normalize_language(language);
            return Ok(mine
                .into_iter()
                .find(|s| normalize_language(&s.language) == language));
        }
        let languages: std::collections::BTreeSet<String> = mine
            .iter()
            .map(|s| normalize_language(&s.language))
            .collect();
        if languages.len() > 1 {
            bail!(
                "this project has R and Python sessions; say which with `language` (r or python) or `session`"
            );
        }
        Ok(mine.into_iter().next())
    }

    async fn list_sessions(&self) -> Result<Vec<Value>> {
        let client = self.client().await?;
        let sessions = self.sessions(&client).await?;
        if sessions.is_empty() {
            return Ok(vec![text_item(
                "No R or Python sessions are running for this project. run_code starts one when it needs it.",
            )]);
        }
        let lines: Vec<String> = sessions.iter().map(describe_session).collect();
        Ok(vec![text_item(&format!(
            "Sessions running for this project, most recently used first:\n{}",
            lines.join("\n")
        ))])
    }

    async fn list_variables(
        &self,
        session: Option<&str>,
        language: Option<&str>,
    ) -> Result<Vec<Value>> {
        let client = self.client().await?;
        let session = self
            .pick(&client, session, language)
            .await?
            .ok_or_else(|| anyhow!("no session is running for that; nothing to list"))?;
        if session.pending_runs > 0 {
            bail!(
                "session {} is busy running code; its variables can be listed once it is idle",
                short_id(&session.id)
            );
        }
        let listed = client.list_variables(&session.id, &[]).await?;
        // Functions are the user's tools, not their data.
        let list: Vec<_> = listed
            .variables
            .into_iter()
            .filter(|v| v.kind != "function")
            .collect();
        let head = format!("Variables in {}:", describe_session(&session));
        if list.is_empty() {
            return Ok(vec![text_item(&format!("{head}\n(none)"))]);
        }
        Ok(vec![text_item(&format!(
            "{head}\n{}",
            format_variables(&list)
        ))])
    }

    async fn run_code(
        &self,
        code: &str,
        session: Option<&str>,
        language: Option<&str>,
        timeout: Duration,
    ) -> Result<Vec<Value>> {
        let mut client = self.client().await?;
        let session = match self.pick(&client, session, language).await? {
            Some(session) => session,
            None => {
                let language = language.context(
                    "no session is running for this project; say which `language` to start (r or python)",
                )?;
                self.start_session(&client, language).await?
            }
        };
        let outcome = run(
            &mut client,
            &session,
            code,
            None,
            self.agent_name(),
            timeout,
        )
        .await?;
        Ok(report(
            &client,
            &format!("Ran in {}", describe_session(&session)),
            &outcome,
        )
        .await)
    }

    /// The agent these tools serve, as the Console marks its runs.
    fn agent_name(&self) -> Option<&'static str> {
        self.terminal_dir
            .as_deref()
            .and_then(crate::terminal::agent_name)
            .or(Some("Agent"))
    }

    async fn start_session(&self, client: &Client, language: &str) -> Result<SessionInfo> {
        let cwd = self.project.clone().or_else(|| std::env::var("HOME").ok());
        self.start_kernel(client, language, None, cwd).await
    }

    /// Starts `kernel` if it is installed, else the usual kernel for
    /// `language`, in `cwd`.
    async fn start_kernel(
        &self,
        client: &Client,
        language: &str,
        kernel: Option<&str>,
        cwd: Option<String>,
    ) -> Result<SessionInfo> {
        let language = normalize_language(language);
        let specs: Vec<KernelspecInfo> = client.list_kernelspecs().await?;
        let preferred: &[&str] = if language == "r" {
            &["ark", "ir"]
        } else {
            &["python3"]
        };
        let spec = kernel
            .and_then(|name| specs.iter().find(|s| s.name == name))
            .or_else(|| {
                preferred
                    .iter()
                    .find_map(|name| specs.iter().find(|s| s.name == *name))
            })
            .or_else(|| {
                specs
                    .iter()
                    .find(|s| normalize_language(&s.language) == language)
            })
            .with_context(|| format!("no {language} kernel is installed on this machine"))?;
        Ok(client
            .call(Call::StartSession {
                kernel: spec.name.clone(),
                cwd,
                project: self.project.clone(),
            })
            .await?)
    }

    /// A file the agent named, relative to the project or absolute.
    fn path(&self, file: &str) -> String {
        let path = match (Path::new(file).is_absolute(), &self.project) {
            (false, Some(project)) if !file.starts_with('~') => Path::new(project).join(file),
            _ => PathBuf::from(file),
        };
        crate::windows::normalize(&path)
            .to_string_lossy()
            .into_owned()
    }

    /// `file` as the agent knows it: relative to the project when inside it.
    fn shown(&self, file: &str) -> String {
        match &self.project {
            Some(project) => Path::new(file)
                .strip_prefix(project)
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|_| file.to_string()),
            None => file.to_string(),
        }
    }

    async fn get_outputs(&self, file: &str, cell: Option<&str>) -> Result<Vec<Value>> {
        let client = self.client().await?;
        let path = self.path(file);
        let outputs: FileOutputs = client.file_outputs(&path).await?;
        let cells: Vec<_> = outputs
            .cells
            .iter()
            .filter(|c| cell.is_none_or(|id| c.cell_id == id))
            .collect();
        if cells.is_empty() {
            return Ok(vec![text_item(&match cell {
                Some(id) => format!("{file}: cell {id} has no saved outputs (it hasn't run)."),
                None => format!("{file}: no cell has run yet, so there are no outputs."),
            })]);
        }
        let mut rendered = Rendered {
            text: format!(
                "{file}: outputs of the last run of {} cell{}\n",
                cells.len(),
                if cells.len() == 1 { "" } else { "s" }
            ),
            ..Default::default()
        };
        for cell in cells {
            let mut head = format!("\n## {} ({})", cell.cell_id, cell.language);
            match (&cell.status, cell.finished_ms) {
                (Some(status), Some(finished)) => head.push_str(&format!(
                    " · {status} · took {} · {} ago",
                    text::duration_ms(finished - cell.started_ms),
                    ago(now_ms() - finished)
                )),
                _ => head.push_str(" · still running"),
            }
            rendered.text.push_str(&head);
            rendered.text.push('\n');
            let before = rendered.text.len();
            for output in &cell.outputs {
                rendered.add_output(output);
            }
            if rendered.text.len() == before {
                rendered.text.push_str("(no output)\n");
            }
        }
        let mut items = vec![text_item(&rendered.finished_text())];
        items.extend(images(&client, &rendered).await);
        Ok(items)
    }
}

/// What a run printed and how it ended.
#[derive(Default)]
struct Outcome {
    rendered: Rendered,
    stdout: String,
    status: Option<String>,
    finished: bool,
    elapsed_ms: i64,
}

/// Runs an agent's code in a session, marked as `by`'s, and collects its
/// output until it finishes or `timeout` passes. With `cell`, the run
/// belongs to that document cell.
async fn run(
    client: &mut Client,
    session: &SessionInfo,
    code: &str,
    cell: Option<CellRef>,
    by: Option<&str>,
    timeout: Duration,
) -> Result<Outcome> {
    client.subscribe(&session.id, None).await?;
    let started = Instant::now();
    let started_run: sandhill_proto::RunStarted = client
        .call(Call::Execute {
            session: session.id.clone(),
            code: code.into(),
            cell,
            quiet: false,
            by: by.map(Into::into),
        })
        .await?;
    let run = started_run.run;
    let mut outcome = Outcome::default();
    follow(
        client,
        &run,
        &mut outcome,
        tokio::time::Instant::now() + timeout,
    )
    .await?;
    let _ = client.unsubscribe(&session.id).await;
    outcome.elapsed_ms = started.elapsed().as_millis() as i64;
    Ok(outcome)
}

/// Collects the events of `run` from the client's subscriptions until it
/// finishes or `deadline` passes.
async fn follow(
    client: &mut Client,
    run: &str,
    outcome: &mut Outcome,
    deadline: tokio::time::Instant,
) -> Result<()> {
    loop {
        let event = match tokio::time::timeout_at(deadline, client.next_event()).await {
            Ok(Some(event)) => event,
            Ok(None) => bail!("lost the connection to sandhilld"),
            Err(_) => return Ok(()),
        };
        if event.run.as_deref() != Some(run) {
            continue;
        }
        if event.msg_type == EVENT_RUN_FINISHED {
            let finished: RunFinished = serde_json::from_value(event.content)?;
            outcome.status = Some(finished.status);
            outcome.finished = true;
            return Ok(());
        }
        if event.msg_type == "stream" && event.content["name"] == "stdout" {
            outcome
                .stdout
                .push_str(event.content["text"].as_str().unwrap_or(""));
        }
        outcome
            .rendered
            .add_message(&event.msg_type, &event.content);
    }
}

/// One line per variable, aligned, for an agent to read.
fn format_variables(variables: &[sandhill_proto::Variable]) -> String {
    let width =
        |f: fn(&sandhill_proto::Variable) -> usize| variables.iter().map(f).max().unwrap_or(0);
    let name_width = width(|v| v.name.chars().count());
    let class_width = width(|v| v.class.chars().count());
    variables
        .iter()
        .map(|v| {
            let size = v.size.map(text::bytes).unwrap_or_default();
            format!(
                "{:name_width$}  {:class_width$}  {:>8}  {}",
                v.name, v.class, size, v.summary
            )
            .trim_end()
            .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// How long a tool waits for code to finish.
fn timeout(args: &Value) -> Duration {
    args["timeout_seconds"]
        .as_f64()
        .map(|s| Duration::from_secs_f64(s.max(1.0)).min(MAX_TIMEOUT))
        .unwrap_or(DEFAULT_TIMEOUT)
}

/// The plots of a reply as MCP image items, fetching stored ones.
async fn images(client: &Client, rendered: &Rendered) -> Vec<Value> {
    let mut items = Vec::new();
    for (mime, value) in rendered.images.iter().take(text::MAX_IMAGES) {
        let data = match value {
            Value::String(data) => Some(data.clone()),
            _ => match value.get(BLOB_KEY).and_then(Value::as_str) {
                Some(hash) => client.get_blob(hash).await.ok().map(|b| b.data),
                None => None,
            },
        };
        let Some(data) = data.map(|d| d.split_whitespace().collect::<String>()) else {
            continue;
        };
        if data.len() > MAX_IMAGE {
            items.push(text_item(&format!(
                "(a plot of {} was left out: too large to send)",
                text::bytes(data.len() as u64 * 3 / 4)
            )));
            continue;
        }
        items.push(json!({"type": "image", "data": data, "mimeType": mime}));
    }
    items
}

/// The tools that only look (`readOnlyHint`), which agents may use without asking.
pub fn read_only_tools() -> Vec<String> {
    let tools = tool_list();
    tools
        .as_array()
        .into_iter()
        .flatten()
        .filter(|t| t["annotations"]["readOnlyHint"] == true)
        .filter_map(|t| t["name"].as_str().map(String::from))
        .collect()
}

fn tool_list() -> Value {
    json!([
        {
            "name": "list_sessions",
            "title": "List R and Python sessions",
            "description": "List the R and Python sessions (Jupyter kernels) running in Sandhill for this project: id, language and version, kernel, whether it is busy, and its working folder.",
            "inputSchema": {"type": "object", "properties": {}},
            "annotations": {"readOnlyHint": true},
        },
        {
            "name": "list_variables",
            "title": "List variables in a session",
            "description": "List the variables in one of the user's live R or Python sessions: class, memory size and a short summary (dimensions and columns of data frames; cells, features, assays and reductions of Seurat, SingleCellExperiment and AnnData objects). Doesn't change the session.",
            "inputSchema": {"type": "object", "properties": {
                "language": {"type": "string", "enum": ["r", "python"], "description": "Which language's session, when the project has both."},
                "session": {"type": "string", "description": "A session id (or its start) from list_sessions, instead of language."},
            }},
            "annotations": {"readOnlyHint": true},
        },
        {
            "name": "run_code",
            "title": "Run code in a session",
            "description": "Run R or Python code in one of the user's live sessions in Sandhill and return what it printed, its value, errors and plots. The user sees it in the session's Console, marked as yours. The objects it creates stay in the user's session. Without `session`, the project's most recently used session for `language` runs it, and one is started if there is none. Code that runs longer than `timeout_seconds` keeps running in the session.",
            "inputSchema": {"type": "object", "required": ["code"], "properties": {
                "code": {"type": "string", "description": "The code to run."},
                "language": {"type": "string", "enum": ["r", "python"]},
                "session": {"type": "string", "description": "A session id (or its start) from list_sessions."},
                "timeout_seconds": {"type": "number", "description": "How long to wait for it to finish (default 120)."},
            }},
            "annotations": {"readOnlyHint": false, "destructiveHint": false},
        },
        {
            "name": "get_focus",
            "title": "See where the user is",
            "description": "Where the user is in Sandhill: the document in front, its current cell (the chunk with the cursor, or the selected notebook cell) with that cell's code, the cursor line, the selected text, the documents open, and the document this conversation is about. Use it when the user says \"this cell\", \"here\" or \"the selection\".",
            "inputSchema": {"type": "object", "properties": {}},
            "annotations": {"readOnlyHint": true},
        },
        {
            "name": "add_cell",
            "title": "Add code to the user's document",
            "description": "Write code into the user's document as a new code cell and run it there, so the user sees the code and its output together, as if they had written it. By default the cell goes into the document this conversation is about (else the one in front), right below the user's current cell (else after the last cell). Runs in the document's own R or Python session and returns the output, errors and plots. A Quarto/R Markdown chunk, a notebook cell, or for a script, code at its end. If the document is open in Sandhill, the edit happens in the editor, so the user can undo it. When the user has chosen to have your code go to the Console, the code runs in the Console in the document's session instead, and the document is left as it is.",
            "inputSchema": {"type": "object", "required": ["code"], "properties": {
                "code": {"type": "string", "description": "The cell's code, without fences."},
                "file": {"type": "string", "description": "The document, relative to the project folder or absolute."},
                "after": {"type": "string", "description": "Put the cell after this cell id (a chunk label, chunk-<n>, or notebook cell id)."},
                "language": {"type": "string", "enum": ["r", "python"], "description": "For a new chunk in a Quarto/R Markdown document; by default the language of the cell before it."},
                "run": {"type": "boolean", "description": "Run the cell (default true)."},
                "timeout_seconds": {"type": "number", "description": "How long to wait for it to finish (default 120)."},
            }},
            "annotations": {"readOnlyHint": false, "destructiveHint": false},
        },
        {
            "name": "edit_cell",
            "title": "Change a cell in the user's document",
            "description": "Replace the code of a cell in the user's document and run it again, for fixing a cell that failed or changing one the user points at. Same defaults as add_cell for the document.",
            "inputSchema": {"type": "object", "required": ["cell", "code"], "properties": {
                "cell": {"type": "string", "description": "The cell id: a chunk label, chunk-<n>, or notebook cell id (as add_cell and get_focus give them)."},
                "code": {"type": "string", "description": "The cell's new code."},
                "file": {"type": "string", "description": "The document, relative to the project folder or absolute."},
                "run": {"type": "boolean", "description": "Run the cell (default true)."},
                "timeout_seconds": {"type": "number", "description": "How long to wait for it to finish (default 120)."},
            }},
            "annotations": {"readOnlyHint": false, "destructiveHint": true},
        },
        {
            "name": "get_outputs",
            "title": "Read a document's outputs",
            "description": "Read the outputs a Quarto (.qmd), R Markdown (.Rmd), Jupyter (.ipynb) document or R/Python script got the last time each of its cells ran in Sandhill: printed text, values, errors and plots. Cells are named by their chunk label (or chunk-<n>) or notebook cell id.",
            "inputSchema": {"type": "object", "required": ["file"], "properties": {
                "file": {"type": "string", "description": "The document, relative to the project folder or absolute."},
                "cell": {"type": "string", "description": "Only this cell."},
            }},
            "annotations": {"readOnlyHint": true},
        },
    ])
}

/// A run's result for the agent: `head`, how it ended, what it printed,
/// and its plots.
async fn report(client: &Client, head: &str, outcome: &Outcome) -> Vec<Value> {
    let status = match (&outcome.status, outcome.finished) {
        (Some(status), _) => status.clone(),
        (None, true) => "unknown".into(),
        (None, false) => format!(
            "still running after {}; it keeps running in the session",
            text::duration_ms(outcome.elapsed_ms)
        ),
    };
    let mut items = vec![text_item(&format!(
        "{head} · {status} · {}\n{}",
        text::duration_ms(outcome.elapsed_ms),
        outcome.rendered.finished_text()
    ))];
    items.extend(images(client, &outcome.rendered).await);
    items
}

fn text_item(text: &str) -> Value {
    json!({"type": "text", "text": text})
}

/// "R session 3f2a9c1e (R 4.4.1, kernel ark) · idle · in /data/pbmc"
fn describe_session(session: &SessionInfo) -> String {
    let language = match normalize_language(&session.language).as_str() {
        "r" => "R".to_string(),
        "python" => "Python".to_string(),
        other => other.to_string(),
    };
    let version = session
        .language_version
        .as_deref()
        .map(|v| format!(" {v}"))
        .unwrap_or_default();
    let state = if session.pending_runs > 0 {
        "busy"
    } else {
        "idle"
    };
    format!(
        "{language} session {} ({language}{version}, kernel {}) · {state} · in {}",
        short_id(&session.id),
        session.kernel,
        session.cwd
    )
}

fn short_id(id: &str) -> &str {
    &id[..id.len().min(8)]
}

fn normalize_language(language: &str) -> String {
    match language.trim().to_ascii_lowercase().as_str() {
        "py" | "python3" | "python" => "python".into(),
        other => other.into(),
    }
}

fn ago(ms: i64) -> String {
    let minutes = ms.max(0) / 60_000;
    match minutes {
        0 => "moments".into(),
        1..=59 => format!("{minutes} min"),
        60..=2879 => format!("{} h", minutes / 60),
        _ => format!("{} days", minutes / 1440),
    }
}
