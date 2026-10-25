//! Durable record of sessions, runs and every output, in SQLite.
//!
//! The journal is what lets a client disconnect (laptop closed, network
//! dropped) and later see everything that happened: outputs are written here
//! as they arrive, whether or not anyone is listening.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{Connection, OptionalExtension, params};
use sandhill_proto::{Event, SessionInfo, SessionStatus};
use serde_json::Value;

use crate::blobs::BlobStore;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS sessions (
    id              TEXT PRIMARY KEY,
    kernel          TEXT NOT NULL,
    language        TEXT NOT NULL,
    cwd             TEXT NOT NULL,
    connection_file TEXT NOT NULL,
    client_session  TEXT NOT NULL,
    pid             INTEGER,
    interrupt_mode  TEXT NOT NULL,
    status          TEXT NOT NULL,
    created_ms      INTEGER NOT NULL,
    ended_ms        INTEGER
);
CREATE TABLE IF NOT EXISTS runs (
    id              TEXT PRIMARY KEY,
    session_id      TEXT NOT NULL REFERENCES sessions(id),
    msg_id          TEXT NOT NULL UNIQUE,
    code            TEXT NOT NULL,
    cell_file       TEXT,
    cell_id         TEXT,
    started_ms      INTEGER NOT NULL,
    finished_ms     INTEGER,
    status          TEXT,
    execution_count INTEGER
);
CREATE TABLE IF NOT EXISTS events (
    seq        INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL REFERENCES sessions(id),
    run_id     TEXT,
    msg_type   TEXT NOT NULL,
    ts_ms      INTEGER NOT NULL,
    content    TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS events_by_session ON events(session_id, seq);
CREATE INDEX IF NOT EXISTS events_by_run ON events(run_id, seq);
CREATE INDEX IF NOT EXISTS runs_by_cell ON runs(cell_file, cell_id);
";

/// Changes to the schema after the first release, applied in order. The
/// database's `user_version` records how many have run.
const MIGRATIONS: &[&str] = &[
    // 1: projects, and the project each session belongs to.
    "ALTER TABLE sessions ADD COLUMN project TEXT;
     CREATE TABLE projects (
         path      TEXT PRIMARY KEY,
         opened_ms INTEGER NOT NULL,
         state     TEXT
     );",
    // 2: the language version the kernel reports, like 4.4.1.
    "ALTER TABLE sessions ADD COLUMN language_version TEXT;",
    // 3: quiet runs (tools looking around) and the agent that ran a run,
    // for the Console, which lists a session's runs.
    "ALTER TABLE runs ADD COLUMN quiet INTEGER NOT NULL DEFAULT 0;
     ALTER TABLE runs ADD COLUMN origin TEXT;
     CREATE INDEX runs_by_session ON runs(session_id);",
];

#[derive(Debug, thiserror::Error)]
pub enum JournalError {
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, JournalError>;

/// What the daemon needs to know to reattach to a kernel after a restart.
#[derive(Debug, Clone)]
pub struct SessionRecord {
    pub id: String,
    pub kernel: String,
    pub language: String,
    pub cwd: String,
    pub connection_file: PathBuf,
    /// Jupyter session id and ZeroMQ identity the daemon uses with this kernel.
    pub client_session: String,
    pub pid: Option<u32>,
    pub interrupt_mode: String,
    pub status: SessionStatus,
    pub created_ms: i64,
    pub ended_ms: Option<i64>,
    pub project: Option<String>,
    pub language_version: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RunRecord {
    pub id: String,
    pub session_id: String,
    pub msg_id: String,
    pub finished: bool,
}

/// What starts a run's record.
#[derive(Debug, Clone, Default)]
pub struct NewRun<'a> {
    pub cell: Option<&'a sandhill_proto::CellRef>,
    /// A tool's run, kept out of the Console.
    pub quiet: bool,
    /// The agent that ran it.
    pub by: Option<&'a str>,
}

/// One of a session's runs, for its Console.
#[derive(Debug, Clone)]
pub struct ConsoleRun {
    pub run_id: String,
    pub cell: Option<sandhill_proto::CellRef>,
    pub by: Option<String>,
    pub code: String,
    pub started_ms: i64,
    pub finished_ms: Option<i64>,
    pub status: Option<String>,
    pub execution_count: Option<u64>,
}

/// An output with an image, and the run that showed it.
#[derive(Debug, Clone)]
pub struct PlotRecord {
    pub run_id: String,
    pub seq: i64,
    pub ts_ms: i64,
    pub content: Value,
    pub cell: Option<sandhill_proto::CellRef>,
    pub by: Option<String>,
    pub code: String,
}

/// A run that came from a notebook cell.
#[derive(Debug, Clone)]
pub struct CellRun {
    pub cell_file: String,
    pub cell_id: String,
    pub run_id: String,
    pub session_id: String,
    pub language: String,
    pub kernel: String,
    pub code: String,
    pub started_ms: i64,
    pub finished_ms: Option<i64>,
    pub status: Option<String>,
    pub execution_count: Option<u64>,
}

pub struct Journal {
    conn: Mutex<Connection>,
    blobs: BlobStore,
}

impl Journal {
    pub fn open(db_path: &Path, blobs: BlobStore) -> Result<Self> {
        let conn = Connection::open(db_path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.execute_batch(SCHEMA)?;
        migrate(&conn)?;
        Ok(Journal {
            conn: Mutex::new(conn),
            blobs,
        })
    }

    pub fn blobs(&self) -> &BlobStore {
        &self.blobs
    }

    pub fn insert_session(&self, s: &SessionRecord) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "INSERT INTO sessions (id, kernel, language, cwd, connection_file, client_session,
                                   pid, interrupt_mode, status, created_ms, ended_ms, project,
                                   language_version)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                s.id,
                s.kernel,
                s.language,
                s.cwd,
                s.connection_file.to_string_lossy(),
                s.client_session,
                s.pid,
                s.interrupt_mode,
                status_str(s.status),
                s.created_ms,
                s.ended_ms,
                s.project,
                s.language_version,
            ],
        )?;
        Ok(())
    }

    pub fn set_language_version(&self, id: &str, version: &str) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "UPDATE sessions SET language_version = ?2 WHERE id = ?1",
            params![id, version],
        )?;
        Ok(())
    }

    pub fn set_session_status(&self, id: &str, status: SessionStatus) -> Result<()> {
        let ended_ms = (status == SessionStatus::Ended).then(now_ms);
        self.conn.lock().unwrap().execute(
            "UPDATE sessions SET status = ?2, ended_ms = COALESCE(?3, ended_ms) WHERE id = ?1",
            params![id, status_str(status), ended_ms],
        )?;
        Ok(())
    }

    pub fn sessions(&self) -> Result<Vec<SessionRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, kernel, language, cwd, connection_file, client_session, pid,
                    interrupt_mode, status, created_ms, ended_ms, project, language_version
             FROM sessions ORDER BY created_ms",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(SessionRecord {
                id: row.get(0)?,
                kernel: row.get(1)?,
                language: row.get(2)?,
                cwd: row.get(3)?,
                connection_file: PathBuf::from(row.get::<_, String>(4)?),
                client_session: row.get(5)?,
                pid: row.get(6)?,
                interrupt_mode: row.get(7)?,
                status: parse_status(&row.get::<_, String>(8)?),
                created_ms: row.get(9)?,
                ended_ms: row.get(10)?,
                project: row.get(11)?,
                language_version: row.get(12)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn session_info(&self, record: &SessionRecord) -> Result<SessionInfo> {
        Ok(SessionInfo {
            id: record.id.clone(),
            kernel: record.kernel.clone(),
            language: record.language.clone(),
            cwd: record.cwd.clone(),
            pid: record.pid,
            status: record.status,
            created_ms: record.created_ms,
            ended_ms: record.ended_ms,
            last_seq: self.last_seq(&record.id)?,
            project: record.project.clone(),
            pending_runs: match record.status {
                SessionStatus::Ended => 0,
                _ => self.unfinished_run_count(&record.id)?,
            },
            language_version: record.language_version.clone(),
        })
    }

    /// Records that `path` was opened as a project and returns its saved state.
    pub fn open_project(&self, path: &str) -> Result<Value> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO projects (path, opened_ms) VALUES (?1, ?2)
             ON CONFLICT(path) DO UPDATE SET opened_ms = excluded.opened_ms",
            params![path, now_ms()],
        )?;
        let state: Option<String> = conn.query_row(
            "SELECT state FROM projects WHERE path = ?1",
            params![path],
            |row| row.get(0),
        )?;
        Ok(match state {
            Some(text) => serde_json::from_str(&text)?,
            None => Value::Null,
        })
    }

    /// `(path, opened_ms)` of every recorded project, most recent first.
    pub fn recent_projects(&self) -> Result<Vec<(String, i64)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT path, opened_ms FROM projects ORDER BY opened_ms DESC, path")?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn save_project_state(&self, path: &str, state: &Value) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "INSERT INTO projects (path, opened_ms, state) VALUES (?1, ?2, ?3)
             ON CONFLICT(path) DO UPDATE SET state = excluded.state",
            params![path, now_ms(), serde_json::to_string(state)?],
        )?;
        Ok(())
    }

    pub fn forget_project(&self, path: &str) -> Result<()> {
        self.conn
            .lock()
            .unwrap()
            .execute("DELETE FROM projects WHERE path = ?1", params![path])?;
        Ok(())
    }

    pub fn last_seq(&self, session: &str) -> Result<i64> {
        Ok(self.conn.lock().unwrap().query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM events WHERE session_id = ?1",
            [session],
            |row| row.get(0),
        )?)
    }

    pub fn insert_run(
        &self,
        id: &str,
        session_id: &str,
        msg_id: &str,
        code: &str,
        run: NewRun,
    ) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "INSERT INTO runs (id, session_id, msg_id, code, cell_file, cell_id, started_ms,
                               quiet, origin)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                id,
                session_id,
                msg_id,
                code,
                run.cell.map(|c| c.file.as_str()),
                run.cell.map(|c| c.cell_id.as_str()),
                now_ms(),
                run.quiet,
                run.by,
            ],
        )?;
        Ok(())
    }

    /// The session's latest `limit` runs that weren't quiet, oldest first.
    pub fn console_runs(&self, session_id: &str, limit: u32) -> Result<Vec<ConsoleRun>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, cell_file, cell_id, origin, code, started_ms, finished_ms, status,
                    execution_count
             FROM runs WHERE session_id = ?1 AND quiet = 0
             ORDER BY rowid DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![session_id, limit], |row| {
            let file: Option<String> = row.get(1)?;
            let cell_id: Option<String> = row.get(2)?;
            Ok(ConsoleRun {
                run_id: row.get(0)?,
                cell: file
                    .zip(cell_id)
                    .map(|(file, cell_id)| sandhill_proto::CellRef { file, cell_id }),
                by: row.get(3)?,
                code: row.get(4)?,
                started_ms: row.get(5)?,
                finished_ms: row.get(6)?,
                status: row.get(7)?,
                execution_count: row.get::<_, Option<i64>>(8)?.map(|n| n as u64),
            })
        })?;
        let mut runs = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        runs.reverse();
        Ok(runs)
    }

    /// A session's latest outputs that hold an image, oldest first, leaving
    /// out quiet runs.
    pub fn session_plots(&self, session_id: &str, limit: u32) -> Result<Vec<PlotRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT e.run_id, e.seq, e.ts_ms, e.content, r.cell_file, r.cell_id, r.origin, r.code
             FROM events e JOIN runs r ON r.id = e.run_id
             WHERE e.session_id = ?1 AND r.quiet = 0
               AND e.msg_type IN ('display_data', 'execute_result')
               AND e.content LIKE '%\"image/%'
             ORDER BY e.seq DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![session_id, limit], |row| {
            let file: Option<String> = row.get(4)?;
            let cell_id: Option<String> = row.get(5)?;
            Ok((
                PlotRecord {
                    run_id: row.get(0)?,
                    seq: row.get(1)?,
                    ts_ms: row.get(2)?,
                    content: Value::Null,
                    cell: file
                        .zip(cell_id)
                        .map(|(file, cell_id)| sandhill_proto::CellRef { file, cell_id }),
                    by: row.get(6)?,
                    code: row.get(7)?,
                },
                row.get::<_, String>(3)?,
            ))
        })?;
        let mut plots = Vec::new();
        for row in rows {
            let (mut plot, content) = row?;
            plot.content = serde_json::from_str(&content)?;
            plots.push(plot);
        }
        plots.reverse();
        Ok(plots)
    }

    pub fn finish_run(&self, id: &str, status: &str, execution_count: Option<u64>) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "UPDATE runs SET finished_ms = ?2, status = ?3, execution_count = ?4 WHERE id = ?1",
            params![id, now_ms(), status, execution_count.map(|n| n as i64)],
        )?;
        Ok(())
    }

    pub fn run_by_msg_id(&self, msg_id: &str) -> Result<Option<RunRecord>> {
        Ok(self
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT id, session_id, msg_id, finished_ms IS NOT NULL FROM runs WHERE msg_id = ?1",
                [msg_id],
                |row| {
                    Ok(RunRecord {
                        id: row.get(0)?,
                        session_id: row.get(1)?,
                        msg_id: row.get(2)?,
                        finished: row.get(3)?,
                    })
                },
            )
            .optional()?)
    }

    /// How many of a session's runs have not finished (running or queued),
    /// leaving out quiet ones: a variable listing doesn't make it busy.
    pub fn unfinished_run_count(&self, session_id: &str) -> Result<u32> {
        Ok(self.conn.lock().unwrap().query_row(
            "SELECT COUNT(*) FROM runs WHERE session_id = ?1 AND finished_ms IS NULL AND quiet = 0",
            [session_id],
            |row| row.get(0),
        )?)
    }

    /// Runs that never got a reply, e.g. because the daemon was down.
    pub fn unfinished_runs(&self, session_id: &str) -> Result<Vec<RunRecord>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, session_id, msg_id FROM runs
             WHERE session_id = ?1 AND finished_ms IS NULL ORDER BY started_ms",
        )?;
        let rows = stmt.query_map([session_id], |row| {
            Ok(RunRecord {
                id: row.get(0)?,
                session_id: row.get(1)?,
                msg_id: row.get(2)?,
                finished: false,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Stores an event, moving large values to the blob store first.
    pub fn append(
        &self,
        session_id: &str,
        run_id: Option<&str>,
        msg_type: &str,
        mut content: Value,
    ) -> Result<Event> {
        self.blobs.externalize(msg_type, &mut content)?;
        let ts_ms = now_ms();
        let text = serde_json::to_string(&content)?;
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO events (session_id, run_id, msg_type, ts_ms, content)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![session_id, run_id, msg_type, ts_ms, text],
        )?;
        Ok(Event {
            seq: conn.last_insert_rowid(),
            session: session_id.to_string(),
            run: run_id.map(str::to_string),
            msg_type: msg_type.to_string(),
            ts_ms,
            content,
        })
    }

    /// Highest sequence number in the whole journal.
    pub fn max_seq(&self) -> Result<i64> {
        Ok(self.conn.lock().unwrap().query_row(
            "SELECT COALESCE(MAX(seq), 0) FROM events",
            [],
            |row| row.get(0),
        )?)
    }

    /// The most recent run of each cell of `file`.
    pub fn latest_cell_runs(&self, file: &str) -> Result<Vec<CellRun>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "{CELL_RUN_SELECT}
             WHERE r.cell_file = ?1
               AND r.rowid = (SELECT MAX(rowid) FROM runs
                              WHERE cell_file = r.cell_file AND cell_id = r.cell_id)
             ORDER BY r.rowid"
        ))?;
        let rows = stmt.query_map([file], cell_run_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn cell_run(&self, run_id: &str) -> Result<Option<CellRun>> {
        let conn = self.conn.lock().unwrap();
        Ok(conn
            .query_row(
                &format!("{CELL_RUN_SELECT} WHERE r.id = ?1 AND r.cell_file IS NOT NULL"),
                [run_id],
                cell_run_from_row,
            )
            .optional()?)
    }

    /// `(msg_type, content)` of a run's events up to and including `max_seq`.
    /// Removes a run and its events, as for a quiet run once its output is read.
    pub fn forget_run(&self, run_id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM events WHERE run_id = ?1", [run_id])?;
        conn.execute("DELETE FROM runs WHERE id = ?1", [run_id])?;
        Ok(())
    }

    /// The session a run ran in, if the journal has the run.
    pub fn run_session(&self, run_id: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        Ok(conn
            .query_row(
                "SELECT session_id FROM runs WHERE id = ?1",
                [run_id],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub fn run_events(&self, run_id: &str, max_seq: i64) -> Result<Vec<(String, Value)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT msg_type, content FROM events
             WHERE run_id = ?1 AND seq <= ?2 ORDER BY seq",
        )?;
        let rows = stmt.query_map(params![run_id, max_seq], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut events = Vec::new();
        for row in rows {
            let (msg_type, content) = row?;
            events.push((msg_type, serde_json::from_str(&content)?));
        }
        Ok(events)
    }

    pub fn events_since(&self, session_id: &str, since_seq: i64, limit: u32) -> Result<Vec<Event>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT seq, run_id, msg_type, ts_ms, content FROM events
             WHERE session_id = ?1 AND seq > ?2 ORDER BY seq LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![session_id, since_seq, limit], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?;
        let mut events = Vec::new();
        for row in rows {
            let (seq, run, msg_type, ts_ms, content) = row?;
            events.push(Event {
                seq,
                session: session_id.to_string(),
                run,
                msg_type,
                ts_ms,
                content: serde_json::from_str(&content)?,
            });
        }
        Ok(events)
    }
}

const CELL_RUN_SELECT: &str = "
    SELECT r.cell_file, r.cell_id, r.id, r.session_id, s.language, s.kernel, r.code,
           r.started_ms, r.finished_ms, r.status, r.execution_count
    FROM runs r JOIN sessions s ON s.id = r.session_id";

fn cell_run_from_row(row: &rusqlite::Row) -> rusqlite::Result<CellRun> {
    Ok(CellRun {
        cell_file: row.get(0)?,
        cell_id: row.get(1)?,
        run_id: row.get(2)?,
        session_id: row.get(3)?,
        language: row.get(4)?,
        kernel: row.get(5)?,
        code: row.get(6)?,
        started_ms: row.get(7)?,
        finished_ms: row.get(8)?,
        status: row.get(9)?,
        execution_count: row.get::<_, Option<i64>>(10)?.map(|n| n as u64),
    })
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn status_str(status: SessionStatus) -> &'static str {
    match status {
        SessionStatus::Starting => "starting",
        SessionStatus::Running => "running",
        SessionStatus::Ended => "ended",
    }
}

fn parse_status(s: &str) -> SessionStatus {
    match s {
        "starting" => SessionStatus::Starting,
        "running" => SessionStatus::Running,
        _ => SessionStatus::Ended,
    }
}

fn migrate(conn: &Connection) -> Result<()> {
    let done: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    for (version, migration) in (1i64..).zip(MIGRATIONS).skip(done.max(0) as usize) {
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(migration)?;
        tx.pragma_update(None, "user_version", version)?;
        tx.commit()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn journal(dir: &Path) -> Journal {
        Journal::open(&dir.join("j.sqlite"), BlobStore::new(dir.join("blobs"))).unwrap()
    }

    fn session(id: &str) -> SessionRecord {
        SessionRecord {
            id: id.into(),
            kernel: "python3".into(),
            language: "python".into(),
            cwd: "/tmp".into(),
            connection_file: "/tmp/k.json".into(),
            client_session: "c".into(),
            pid: Some(1),
            interrupt_mode: "signal".into(),
            status: SessionStatus::Running,
            created_ms: 0,
            ended_ms: None,
            project: None,
            language_version: None,
        }
    }

    #[test]
    fn projects_are_listed_most_recent_first_with_their_state() {
        let dir = tempfile::tempdir().unwrap();
        let j = journal(dir.path());
        assert_eq!(j.open_project("/w/a").unwrap(), Value::Null);
        j.save_project_state("/w/a", &json!({"tabs": ["/w/a/x.qmd"]}))
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        j.open_project("/w/b").unwrap();
        let paths: Vec<_> = j
            .recent_projects()
            .unwrap()
            .into_iter()
            .map(|p| p.0)
            .collect();
        assert_eq!(paths, ["/w/b", "/w/a"]);
        // Reopening moves it to the top and brings back its state.
        std::thread::sleep(std::time::Duration::from_millis(5));
        assert_eq!(
            j.open_project("/w/a").unwrap(),
            json!({"tabs": ["/w/a/x.qmd"]})
        );
        assert_eq!(j.recent_projects().unwrap()[0].0, "/w/a");
        j.forget_project("/w/a").unwrap();
        assert_eq!(j.recent_projects().unwrap().len(), 1);
    }

    #[test]
    fn journals_from_before_projects_are_upgraded() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("j.sqlite");
        {
            let conn = Connection::open(&db).unwrap();
            conn.execute_batch(SCHEMA).unwrap();
            conn.execute(
                "INSERT INTO sessions (id, kernel, language, cwd, connection_file,
                                       client_session, pid, interrupt_mode, status, created_ms)
                 VALUES ('old', 'ir', 'R', '/', '/k.json', 'c', 1, 'signal', 'running', 0)",
                [],
            )
            .unwrap();
        }
        let j = journal(dir.path());
        let sessions = j.sessions().unwrap();
        assert_eq!(sessions[0].id, "old");
        assert_eq!(sessions[0].project, None);
        let mut tagged = session("new");
        tagged.project = Some("/w/a".into());
        j.insert_session(&tagged).unwrap();
        assert_eq!(j.sessions().unwrap()[1].project.as_deref(), Some("/w/a"));
        // Opening again doesn't run the migrations twice.
        drop(j);
        journal(dir.path());
    }

    #[test]
    fn events_replay_in_order_per_session() {
        let dir = tempfile::tempdir().unwrap();
        let j = journal(dir.path());
        j.insert_session(&session("a")).unwrap();
        j.insert_session(&session("b")).unwrap();
        let first = j.append("a", None, "stream", json!({"text": "1"})).unwrap();
        j.append("b", None, "stream", json!({"text": "other"}))
            .unwrap();
        let third = j.append("a", None, "stream", json!({"text": "2"})).unwrap();

        let all = j.events_since("a", 0, 100).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].content["text"], "1");
        assert_eq!(all[1].seq, third.seq);
        assert_eq!(j.events_since("a", first.seq, 100).unwrap().len(), 1);
        assert_eq!(j.last_seq("a").unwrap(), third.seq);
    }

    #[test]
    fn latest_run_per_cell() {
        let dir = tempfile::tempdir().unwrap();
        let j = journal(dir.path());
        j.insert_session(&session("a")).unwrap();
        let cell = |id: &str| sandhill_proto::CellRef {
            file: "/w/a.qmd".into(),
            cell_id: id.into(),
        };
        j.insert_run("r1", "a", "m1", "1", cell_run(&cell("x")))
            .unwrap();
        j.insert_run("r2", "a", "m2", "2", cell_run(&cell("y")))
            .unwrap();
        j.insert_run("r3", "a", "m3", "3", cell_run(&cell("x")))
            .unwrap();
        j.insert_run("r4", "a", "m4", "4", NewRun::default())
            .unwrap();
        j.finish_run("r3", "ok", Some(3)).unwrap();
        let runs = j.latest_cell_runs("/w/a.qmd").unwrap();
        let ids: Vec<_> = runs
            .iter()
            .map(|r| (r.cell_id.as_str(), r.run_id.as_str()))
            .collect();
        assert_eq!(ids, [("y", "r2"), ("x", "r3")]);
        assert_eq!(runs[1].execution_count, Some(3));
        assert_eq!(runs[1].language, "python");
        assert!(j.cell_run("r4").unwrap().is_none());

        j.append("a", Some("r3"), "stream", json!({"text": "early"}))
            .unwrap();
        let cutoff = j.max_seq().unwrap();
        j.append("a", Some("r3"), "stream", json!({"text": "late"}))
            .unwrap();
        let events = j.run_events("r3", cutoff).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].1["text"], "early");
    }

    fn cell_run(cell: &sandhill_proto::CellRef) -> NewRun<'_> {
        NewRun {
            cell: Some(cell),
            ..Default::default()
        }
    }

    #[test]
    fn the_console_lists_a_sessions_runs_but_not_quiet_ones() {
        let dir = tempfile::tempdir().unwrap();
        let j = journal(dir.path());
        j.insert_session(&session("a")).unwrap();
        j.insert_session(&session("b")).unwrap();
        let cell = sandhill_proto::CellRef {
            file: "/w/a.R".into(),
            cell_id: "console-0".into(),
        };
        j.insert_run("r1", "a", "m1", "x <- 1", cell_run(&cell))
            .unwrap();
        let quiet = NewRun {
            quiet: true,
            ..Default::default()
        };
        j.insert_run("r2", "a", "m2", "ls()", quiet).unwrap();
        let by = NewRun {
            by: Some("Claude"),
            ..Default::default()
        };
        j.insert_run("r3", "a", "m3", "head(x)", by).unwrap();
        j.insert_run("r4", "a", "m4", "y", NewRun::default())
            .unwrap();
        j.insert_run("r5", "b", "m5", "z", NewRun::default())
            .unwrap();
        j.finish_run("r1", "ok", Some(1)).unwrap();
        let runs = j.console_runs("a", 10).unwrap();
        let ids: Vec<_> = runs.iter().map(|r| r.run_id.as_str()).collect();
        assert_eq!(ids, ["r1", "r3", "r4"]);
        assert_eq!(runs[0].cell.as_ref(), Some(&cell));
        assert_eq!(runs[0].execution_count, Some(1));
        assert_eq!(runs[1].by.as_deref(), Some("Claude"));
        let last: Vec<_> = j
            .console_runs("a", 2)
            .unwrap()
            .into_iter()
            .map(|r| r.run_id)
            .collect();
        assert_eq!(last, ["r3", "r4"]);
        // r3 and r4 run; the quiet r2 doesn't count.
        assert_eq!(j.unfinished_run_count("a").unwrap(), 2);
    }

    #[test]
    fn journal_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        {
            let j = journal(dir.path());
            j.insert_session(&session("a")).unwrap();
            j.insert_run("r1", "a", "m1", "print(1)", NewRun::default())
                .unwrap();
            j.append("a", Some("r1"), "stream", json!({"text": "1"}))
                .unwrap();
        }
        let j = journal(dir.path());
        assert_eq!(j.sessions().unwrap().len(), 1);
        assert_eq!(j.unfinished_runs("a").unwrap().len(), 1);
        assert_eq!(j.run_by_msg_id("m1").unwrap().unwrap().id, "r1");
        assert_eq!(
            j.events_since("a", 0, 10).unwrap()[0].run.as_deref(),
            Some("r1")
        );
    }
}
