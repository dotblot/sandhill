//! `sandhilld mcp`: the tools Sandhill gives coding agents, as an MCP
//! server over stdio. Claude Code and Codex start it themselves (Sandhill
//! passes it on their command line), and it talks to the daemon over its
//! socket like any other client, so it works through daemon restarts.
//!
//! The tools reach the user's live sessions, which is what plain file
//! access can't: `list_sessions`, `list_variables`, `run_code` and
//! `get_outputs`.

pub mod text;
pub mod variables;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use sandhill_proto::client::Client;
use sandhill_proto::{
    BLOB_KEY, Call, EVENT_RUN_FINISHED, FileOutputs, KernelspecInfo, RunFinished, SessionInfo,
    SessionStatus,
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
const VARIABLES_TIMEOUT: Duration = Duration::from_secs(30);
/// Plots larger than this (base64) are mentioned but not sent.
const MAX_IMAGE: usize = 2 << 20;

const INSTRUCTIONS: &str = "Sandhill is the IDE the user runs you in. These tools reach the \
user's live R and Python sessions (Jupyter kernels) for this project: the objects they have \
loaded and the outputs of their notebooks. Use list_variables and get_outputs to look before \
recomputing anything; run_code runs code in the user's session, where its results stay.";

pub struct Tools {
    socket: PathBuf,
    /// The project folder the agent works on; sessions of other projects
    /// are left alone unless named.
    project: Option<String>,
}

/// Serves MCP on stdin and stdout until stdin closes.
pub async fn serve(socket: PathBuf, project: Option<String>) -> Result<()> {
    let tools = Arc::new(Tools { socket, project });
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
        Tools { socket, project }
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
                match self.call(name, &args).await {
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

    /// Runs one tool. Returns MCP content items.
    pub async fn call(&self, name: &str, args: &Value) -> Result<Vec<Value>> {
        let text = |s: &str| args[s].as_str().map(str::trim).filter(|s| !s.is_empty());
        match name {
            "list_sessions" => self.list_sessions().await,
            "list_variables" => self.list_variables(text("session"), text("language")).await,
            "run_code" => {
                let code = text("code").context("`code` is required")?;
                let timeout = args["timeout_seconds"]
                    .as_f64()
                    .map(|s| Duration::from_secs_f64(s.max(1.0)).min(MAX_TIMEOUT))
                    .unwrap_or(DEFAULT_TIMEOUT);
                self.run_code(code, text("session"), text("language"), timeout)
                    .await
            }
            "get_outputs" => {
                let file = text("file").context("`file` is required")?;
                self.get_outputs(file, text("cell")).await
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
        let mut client = self.client().await?;
        let session = self
            .pick(&client, session, language)
            .await?
            .ok_or_else(|| anyhow!("no session is running for that; nothing to list"))?;
        let code = variables::code_for(&session.language)
            .with_context(|| format!("cannot list variables of a {} session", session.language))?;
        if session.pending_runs > 0 {
            bail!(
                "session {} is busy running code; its variables can be listed once it is idle",
                short_id(&session.id)
            );
        }
        let outcome = run(&mut client, &session, code, true, VARIABLES_TIMEOUT).await?;
        if !outcome.finished {
            bail!("listing the variables took too long; the session may be busy");
        }
        let list = variables::parse(&outcome.stdout);
        let head = format!("Variables in {}:", describe_session(&session));
        if list.is_empty() {
            let note = outcome.rendered.finished_text();
            return Ok(vec![text_item(&if note.trim().is_empty() {
                format!("{head}\n(none)")
            } else {
                format!("{head}\n(none found; the session printed:)\n{note}")
            })]);
        }
        Ok(vec![text_item(&format!(
            "{head}\n{}",
            variables::format(&list)
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
        let outcome = run(&mut client, &session, code, false, timeout).await?;
        let status = match (&outcome.status, outcome.finished) {
            (Some(status), _) => status.clone(),
            (None, true) => "unknown".into(),
            (None, false) => format!(
                "still running after {}; it keeps running in the session",
                text::duration_ms(outcome.elapsed_ms)
            ),
        };
        let mut items = vec![text_item(&format!(
            "Ran in {} · {} · {}\n{}",
            describe_session(&session),
            status,
            text::duration_ms(outcome.elapsed_ms),
            outcome.rendered.finished_text()
        ))];
        items.extend(images(&client, &outcome.rendered).await);
        Ok(items)
    }

    async fn start_session(&self, client: &Client, language: &str) -> Result<SessionInfo> {
        let language = normalize_language(language);
        let specs: Vec<KernelspecInfo> = client.list_kernelspecs().await?;
        let preferred: &[&str] = if language == "r" {
            &["ark", "ir"]
        } else {
            &["python3"]
        };
        let spec = preferred
            .iter()
            .find_map(|name| specs.iter().find(|s| s.name == *name))
            .or_else(|| {
                specs
                    .iter()
                    .find(|s| normalize_language(&s.language) == language)
            })
            .with_context(|| format!("no {language} kernel is installed on this machine"))?;
        let cwd = self.project.clone().or_else(|| std::env::var("HOME").ok());
        Ok(client
            .call(Call::StartSession {
                kernel: spec.name.clone(),
                cwd,
                project: self.project.clone(),
            })
            .await?)
    }

    async fn get_outputs(&self, file: &str, cell: Option<&str>) -> Result<Vec<Value>> {
        let client = self.client().await?;
        let path = match (Path::new(file).is_absolute(), &self.project) {
            (false, Some(project)) if !file.starts_with('~') => {
                Path::new(project).join(file).to_string_lossy().into_owned()
            }
            _ => file.to_string(),
        };
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
struct Outcome {
    rendered: Rendered,
    stdout: String,
    status: Option<String>,
    finished: bool,
    elapsed_ms: i64,
}

/// Runs code in a session and collects its output until it finishes or
/// `timeout` passes.
async fn run(
    client: &mut Client,
    session: &SessionInfo,
    code: &str,
    quiet: bool,
    timeout: Duration,
) -> Result<Outcome> {
    client.subscribe(&session.id, None).await?;
    let started = Instant::now();
    let run = if quiet {
        client.execute_quietly(&session.id, code).await?
    } else {
        client.execute(&session.id, code, None).await?
    }
    .run;
    let mut outcome = Outcome {
        rendered: Rendered::default(),
        stdout: String::new(),
        status: None,
        finished: false,
        elapsed_ms: 0,
    };
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let event = match tokio::time::timeout_at(deadline, client.next_event()).await {
            Ok(Some(event)) => event,
            Ok(None) => bail!("lost the connection to sandhilld"),
            Err(_) => break,
        };
        if event.run.as_deref() != Some(run.as_str()) {
            continue;
        }
        if event.msg_type == EVENT_RUN_FINISHED {
            let finished: RunFinished = serde_json::from_value(event.content)?;
            outcome.status = Some(finished.status);
            outcome.finished = true;
            break;
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
    let _ = client.unsubscribe(&session.id).await;
    outcome.elapsed_ms = started.elapsed().as_millis() as i64;
    // The variables listing is for the tool, not for the agent to read.
    if quiet {
        outcome.rendered.text = outcome
            .rendered
            .text
            .lines()
            .filter(|l| !l.starts_with(variables::MARK))
            .map(|l| format!("{l}\n"))
            .collect();
    }
    Ok(outcome)
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
            "description": "Run R or Python code in one of the user's live sessions in Sandhill and return what it printed, its value, errors and plots. The objects it creates stay in the user's session. Without `session`, the project's most recently used session for `language` runs it, and one is started if there is none. Code that runs longer than `timeout_seconds` keeps running in the session.",
            "inputSchema": {"type": "object", "required": ["code"], "properties": {
                "code": {"type": "string", "description": "The code to run."},
                "language": {"type": "string", "enum": ["r", "python"]},
                "session": {"type": "string", "description": "A session id (or its start) from list_sessions."},
                "timeout_seconds": {"type": "number", "description": "How long to wait for it to finish (default 120)."},
            }},
            "annotations": {"readOnlyHint": false, "destructiveHint": false},
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
