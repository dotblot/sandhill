//! Agent conversations kept with their project, so they travel with its
//! folder (a portable drive, a synced copy) and go on on another machine.
//!
//! Each conversation runs in a terminal, whose folder is on this machine, as
//! before. When the project keeps its chat history (the default; see
//! `[agents] history` in `project.toml`), what another machine needs is
//! copied to `.sandhill/agents/<id>/`, which git ignores:
//!
//! ```text
//! conversation.json  its title, agent and settings, what finds the agent's
//!                    own conversation again, and the machine it ran on
//! chat.jsonl         the chat, as the Agents pane shows it
//! session.jsonl      the agent's own copy of the conversation (Claude
//!                    Code's transcript, Codex's rollout), to go on with
//! earlier.md         the chat as text, for an agent that can't go on
//! ```
//!
//! Both copies only grow, so the longer one is the newer: [`sync`] copies
//! what one side has beyond the other. A conversation from another machine
//! becomes an ended terminal here ([`adopt`]) when its project is opened.
//! Before one goes on, [`prepare`] puts the agent's copy back where the
//! agent looks for it; when that can't be done, the agent starts afresh and
//! reads `earlier.md` first.
//!
//! Logins never move: each machine signs in to the agents on its own.

use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Result, bail};
use sandhill_proto::{AgentOptions, ChatEntry, ChatHistory, TerminalInfo};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::agents::{self, ANTIGRAVITY, CLAUDE, CODEX};
use crate::journal::now_ms;
use crate::projects::settings;
use crate::terminal::{Adopted, Terminals, chat};

/// Where a project keeps its conversations.
pub const DIR: &str = ".sandhill/agents";
const CONVERSATION: &str = "conversation.json";
const CHAT: &str = "chat.jsonl";
const SESSION: &str = "session.jsonl";
const EARLIER: &str = "earlier.md";

/// How often a machine running a conversation says so, and how long the
/// others take its word for it (a machine can go to sleep or away).
const HEARTBEAT_MS: i64 = 60_000;
const RUNNING_FOR_MS: i64 = 3 * HEARTBEAT_MS;

/// One copy at a time, between the daemon's sync and a window's requests.
static LOCK: Mutex<()> = Mutex::new(());

/// A conversation's `conversation.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Conversation {
    pub id: String,
    pub kind: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub options: AgentOptions,
    pub created_ms: i64,
    /// When its chat last changed.
    pub updated_ms: i64,
    /// What finds the agent's own conversation again: Claude Code's
    /// session, Codex's thread, Antigravity's conversation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume: Option<String>,
    /// The name of the agent's own file that `session.jsonl` copies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_file: Option<String>,
    /// The machine it last ran on.
    #[serde(default)]
    pub machine: String,
    /// While it runs there, when that machine last said so.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub running_ms: Option<i64>,
}

/// Where the agents keep their own conversations on this machine.
#[derive(Debug, Clone)]
pub struct Homes {
    pub claude: PathBuf,
    pub codex: PathBuf,
}

impl Homes {
    pub fn from_env() -> Homes {
        let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
        let var = |name: &str| {
            std::env::var_os(name)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        Homes {
            claude: var("CLAUDE_CONFIG_DIR").unwrap_or_else(|| home.join(".claude")),
            codex: var("CODEX_HOME").unwrap_or_else(|| home.join(".codex")),
        }
    }
}

/// Whether a project keeps its conversations in its folder.
pub fn kept_with(root: &Path) -> bool {
    let history = settings::read(root)
        .ok()
        .flatten()
        .map(|s| s.chat_history)
        .unwrap_or_default();
    history == ChatHistory::Project
}

/// Copies what changed in each conversation of a project that keeps its
/// chats: this machine's news to the project, or another machine's here.
pub fn sync(terminals: &Terminals, homes: &Homes, machine: &str) {
    let _lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut keeps = HashMap::new();
    for info in terminals.list() {
        let Some(project) = info.project.clone() else {
            continue;
        };
        if !agents::is_agent(&info.kind) {
            continue;
        }
        let root = PathBuf::from(&project);
        // A drive that isn't plugged in, or a project that moved.
        if !root.is_dir() || !*keeps.entry(project).or_insert_with(|| kept_with(&root)) {
            continue;
        }
        if let Err(err) = sync_one(terminals, &info, &root, homes, machine) {
            tracing::debug!("conversation {}: {err:#}", info.id);
        }
    }
}

fn sync_one(
    terminals: &Terminals,
    info: &TerminalInfo,
    root: &Path,
    homes: &Homes,
    machine: &str,
) -> Result<()> {
    let now = now_ms();
    let dir = root.join(DIR).join(&info.id);
    let local = chat::path(&terminals.dir(&info.id)?);
    let shared = dir.join(CHAT);
    let theirs: Option<Conversation> = read_json(&dir.join(CONVERSATION));
    let (ours_len, theirs_len) = (len(&local), len(&shared));
    if theirs_len > ours_len {
        // It went on elsewhere: take what it said, unless it runs here too.
        if !info.running {
            append(&shared, &local, ours_len)?;
            if let Some(theirs) = &theirs {
                if let (Some(key), Some(id)) = (agents::resume_key(&info.kind), &theirs.resume) {
                    terminals.set_report(&info.id, key, json!(id))?;
                }
                if theirs.name != info.name {
                    terminals.rename(&info.id, theirs.name.as_deref())?;
                }
            }
        }
        return Ok(());
    }
    // Only one machine writes at a time: the one with news, else the one
    // that wrote last. Two machines on one synced folder don't take turns.
    let news = match &theirs {
        None => ours_len > 0,
        Some(theirs) => ours_len > theirs_len || theirs.machine == machine,
    };
    if !news {
        return Ok(());
    }
    if !dir.exists() {
        let agents = root.join(DIR);
        if !agents.exists() {
            std::fs::create_dir_all(&agents)?;
            settings::ignore(root, "agents/")?;
        }
        std::fs::create_dir_all(&dir)?;
    }
    if ours_len > theirs_len {
        append(&local, &shared, theirs_len)?;
    }

    let resume = agents::resume_id(&info.kind, info.agent.as_ref());
    let same_session = theirs.as_ref().is_some_and(|t| t.resume == resume);
    let mut session_file = theirs
        .as_ref()
        .filter(|_| same_session)
        .and_then(|t| t.session_file.clone());
    if let Some(source) = transcript(info, homes).filter(|p| p.is_file()) {
        let copy = dir.join(SESSION);
        let name = file_name(&source);
        let (have, kept) = (len(&source), len(&copy));
        if !same_session || session_file.as_ref() != Some(&name) || have < kept {
            copy_atomic(&source, &copy)?;
        } else if have > kept {
            append(&source, &copy, kept)?;
        }
        session_file = Some(name);
    }

    let mut ours = Conversation {
        id: info.id.clone(),
        kind: info.kind.clone(),
        title: info.title.clone(),
        name: info.name.clone(),
        options: info.options.clone(),
        created_ms: info.created_ms,
        updated_ms: modified_ms(&local).unwrap_or(now),
        resume,
        session_file,
        machine: machine.to_string(),
        running_ms: info.running.then_some(now),
    };
    // While it runs, say so once a heartbeat, not every time.
    if let Some(theirs) = &theirs
        && info.running
        && theirs.machine == machine
        && theirs.running_ms.is_some_and(|ms| now - ms < HEARTBEAT_MS)
    {
        ours.running_ms = theirs.running_ms;
    }
    if theirs.as_ref() != Some(&ours) {
        crate::files::write_atomic(&dir.join(CONVERSATION), &serde_json::to_vec_pretty(&ours)?)?;
    }
    Ok(())
}

/// Makes a terminal here for each of a project's conversations that ran on
/// another machine, so they show in its history. One whose project moved
/// on this machine follows it.
pub fn adopt(terminals: &Terminals, root: &Path) {
    let _lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(entries) = std::fs::read_dir(root.join(DIR)) else {
        return;
    };
    let project = root.to_string_lossy().into_owned();
    for entry in entries.flatten() {
        let dir = entry.path();
        let id = entry.file_name().to_string_lossy().into_owned();
        let Some(conversation) = read_json::<Conversation>(&dir.join(CONVERSATION)) else {
            continue;
        };
        if conversation.id != id || !agents::is_agent(&conversation.kind) {
            continue;
        }
        if let Ok(info) = terminals.info(&id) {
            let moved = info
                .project
                .as_ref()
                .is_some_and(|p| p != &project && !Path::new(p).exists());
            if moved && !info.running {
                let _ = terminals.move_to(&id, &project);
            }
            continue;
        }
        let chat = dir.join(CHAT);
        if !chat.is_file() {
            continue;
        }
        let mut report = serde_json::Map::new();
        if let (Some(key), Some(resume)) =
            (agents::resume_key(&conversation.kind), &conversation.resume)
        {
            report.insert(key.into(), json!(resume));
        }
        let adopted = Adopted {
            id,
            kind: conversation.kind,
            title: conversation.title,
            name: conversation.name,
            project: project.clone(),
            options: conversation.options,
            created_ms: conversation.created_ms,
            ended_ms: conversation.updated_ms,
            report,
        };
        if let Err(err) = terminals.adopt(adopted, &chat) {
            tracing::warn!("conversation in {}: {err:#}", dir.display());
        }
    }
}

/// Moves a project's conversations out of its folder, when it stops
/// keeping them there: each becomes a terminal here first.
pub fn move_out(terminals: &Terminals, root: &Path) -> Result<()> {
    adopt(terminals, root);
    let _lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = root.join(DIR);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    Ok(())
}

/// Removes the project's copy of a conversation that was removed here.
pub fn forget(root: &Path, id: &str) -> Result<()> {
    let _lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Ok(());
    }
    let dir = root.join(DIR).join(id);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    Ok(())
}

/// How an agent goes on with its conversation.
#[derive(Debug, PartialEq)]
pub struct Resume {
    /// What the agent resumes ([`agents::resume_id`]), if it can.
    pub id: Option<String>,
    /// For an agent that starts afresh: where the earlier chat is, to read
    /// first.
    pub note: Option<String>,
}

/// Gets an agent ready to go on with its conversation, which may have run
/// on another machine: puts the agent's own copy back where it looks for
/// it. Without one, the agent starts afresh with the chat to read.
pub fn prepare(info: &TerminalInfo, homes: &Homes, machine: &str) -> Result<Resume> {
    let _lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let id = agents::resume_id(&info.kind, info.agent.as_ref());
    let as_before = Resume {
        id: id.clone(),
        note: None,
    };
    let Some(project) = &info.project else {
        return Ok(as_before);
    };
    let dir = Path::new(project).join(DIR).join(&info.id);
    let Some(conversation) = read_json::<Conversation>(&dir.join(CONVERSATION)) else {
        return Ok(as_before);
    };
    if runs_elsewhere(&conversation, machine, now_ms()) {
        bail!(
            "This conversation is running on {}. End it there first, or wait a few minutes if that machine is off.",
            conversation.machine
        );
    }
    let Some(id) = id else {
        return Ok(as_before);
    };
    let copy = dir.join(SESSION);
    let here = match info.kind.as_str() {
        CLAUDE => restore(&copy, &claude_transcript(homes, Path::new(&info.cwd), &id)),
        CODEX => {
            let file = codex_rollout(&homes.codex, &id).unwrap_or_else(|| {
                let name = conversation
                    .session_file
                    .clone()
                    .unwrap_or_else(|| format!("rollout-{id}.jsonl"));
                homes
                    .codex
                    .join("sessions")
                    .join(day_of(&name, conversation.created_ms))
                    .join(name)
            });
            restore(&copy, &file)
        }
        // Where agy keeps conversations isn't known: it goes on where it ran.
        ANTIGRAVITY => conversation.machine == machine,
        _ => true,
    };
    if here {
        return Ok(Resume {
            id: Some(id),
            note: None,
        });
    }
    let earlier = dir.join(EARLIER);
    std::fs::write(&earlier, earlier_text(&dir.join(CHAT)))?;
    Ok(Resume {
        id: None,
        note: Some(format!(
            "This conversation began on another computer, where you kept your memory of it. What was said is in {}: read it first, then go on.",
            earlier.display()
        )),
    })
}

/// The first message of an agent that starts afresh: `note`, then the
/// user's message, or a request to say when it has caught up.
pub fn with_note(note: &str, prompt: Option<&str>) -> String {
    match prompt.map(str::trim).filter(|p| !p.is_empty()) {
        Some(prompt) => format!("{note}\n\n{prompt}"),
        None => format!("{note} Then say in a line that you're ready."),
    }
}

fn runs_elsewhere(conversation: &Conversation, machine: &str, now: i64) -> bool {
    conversation.machine != machine
        && conversation
            .running_ms
            .is_some_and(|ms| now - ms < RUNNING_FOR_MS)
}

/// The agent's own copy of the conversation on this machine.
fn transcript(info: &TerminalInfo, homes: &Homes) -> Option<PathBuf> {
    let report = info.agent.as_ref();
    if let Some(path) = report.and_then(|r| r["transcript"].as_str()) {
        return Some(PathBuf::from(path));
    }
    let id = agents::resume_id(&info.kind, report)?;
    match info.kind.as_str() {
        CLAUDE => Some(claude_transcript(homes, Path::new(&info.cwd), &id)),
        CODEX => codex_rollout(&homes.codex, &id),
        _ => None,
    }
}

/// Where Claude Code keeps a session started in `cwd`: in a folder named
/// after `cwd`, every character but letters and digits made a dash.
pub fn claude_transcript(homes: &Homes, cwd: &Path, session: &str) -> PathBuf {
    let folder: String = cwd
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    homes
        .claude
        .join("projects")
        .join(folder)
        .join(format!("{session}.jsonl"))
}

/// Codex's file for a thread: `sessions/<year>/<month>/<day>/rollout-<time>-<thread>.jsonl`.
fn codex_rollout(codex: &Path, thread: &str) -> Option<PathBuf> {
    let suffix = format!("{thread}.jsonl");
    let mut dirs = vec![codex.join("sessions")];
    for _ in 0..3 {
        dirs = dirs
            .iter()
            .filter_map(|d| std::fs::read_dir(d).ok())
            .flat_map(|e| e.flatten().map(|e| e.path()).filter(|p| p.is_dir()))
            .collect();
    }
    dirs.iter()
        .filter_map(|d| std::fs::read_dir(d).ok())
        .flat_map(|e| e.flatten().map(|e| e.path()))
        .find(|p| file_name(p).ends_with(&suffix))
}

/// The `<year>/<month>/<day>` folder for a Codex file: from its name
/// (`rollout-2026-10-28T...`), else from `ms`.
fn day_of(name: &str, ms: i64) -> PathBuf {
    let date = name
        .strip_prefix("rollout-")
        .and_then(|rest| rest.get(..10));
    if let Some(date) = date
        && let [y, m, d] = date.split('-').collect::<Vec<_>>()[..]
        && [y, m, d]
            .iter()
            .all(|p| p.bytes().all(|b| b.is_ascii_digit()))
    {
        return PathBuf::from(y).join(m).join(d);
    }
    let (y, m, d) = civil(ms.div_euclid(86_400_000));
    PathBuf::from(format!("{y:04}"))
        .join(format!("{m:02}"))
        .join(format!("{d:02}"))
}

/// The date of a day counted from 1970-01-01 (Howard Hinnant's algorithm).
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// Puts `copy` at `file` unless `file` has at least as much. Whether the
/// agent has its conversation there now.
fn restore(copy: &Path, file: &Path) -> bool {
    if copy.is_file() && len(file) < len(copy) {
        let done = file
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .map_err(anyhow::Error::from)
            .and_then(|()| copy_atomic(copy, file));
        if let Err(err) = done {
            tracing::warn!("cannot put {} back: {err:#}", file.display());
        }
    }
    file.is_file()
}

/// The chat as text: what the user said and the agent's replies, without
/// its steps.
fn earlier_text(chat: &Path) -> String {
    enum Said {
        User(String),
        Agent(BTreeMap<u64, String>),
    }
    let mut said: Vec<Said> = Vec::new();
    let mut replies: HashMap<String, usize> = HashMap::new();
    let text = std::fs::read_to_string(chat).unwrap_or_default();
    for entry in text
        .lines()
        .filter_map(|l| serde_json::from_str::<ChatEntry>(l).ok())
    {
        let event = &entry.event;
        match event["kind"].as_str() {
            Some("prompt") => said.push(Said::User(event["text"].as_str().unwrap_or("").into())),
            Some("text") => {
                let id = event["message"]
                    .as_str()
                    .map_or_else(|| format!("m{}", entry.seq), String::from);
                let at = *replies.entry(id).or_insert_with(|| {
                    said.push(Said::Agent(BTreeMap::new()));
                    said.len() - 1
                });
                if let Said::Agent(parts) = &mut said[at] {
                    let index = event["index"].as_u64().unwrap_or(parts.len() as u64);
                    parts.insert(index, event["text"].as_str().unwrap_or("").into());
                }
            }
            _ => {}
        }
    }
    let mut out = String::from("# Earlier in this conversation\n");
    for item in said {
        let (who, text) = match item {
            Said::User(text) => ("The user", text),
            Said::Agent(parts) => ("You", parts.into_values().collect()),
        };
        out.push_str(&format!("\n## {who}\n\n{}\n", text.trim()));
    }
    out
}

fn len(path: &Path) -> u64 {
    std::fs::metadata(path).map_or(0, |m| m.len())
}

fn modified_ms(path: &Path) -> Option<i64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    let since = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
    i64::try_from(since.as_millis()).ok()
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// Appends what `from` has past byte `at` to `to`, which has `at` bytes.
fn append(from: &Path, to: &Path, at: u64) -> Result<()> {
    let mut source = std::fs::File::open(from)?;
    source.seek(SeekFrom::Start(at))?;
    let mut rest = Vec::new();
    source.read_to_end(&mut rest)?;
    let mut target = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(to)?;
    target.write_all(&rest)?;
    Ok(())
}

fn copy_atomic(from: &Path, to: &Path) -> Result<()> {
    crate::files::write_atomic(to, &std::fs::read(from)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_names_a_session_folder_after_its_path() {
        let homes = Homes {
            claude: "/h/.claude".into(),
            codex: "/h/.codex".into(),
        };
        assert_eq!(
            claude_transcript(&homes, Path::new("/Volumes/Lab Drive/rna_seq.v2"), "5f3c"),
            Path::new("/h/.claude/projects/-Volumes-Lab-Drive-rna-seq-v2/5f3c.jsonl")
        );
    }

    #[test]
    fn codex_files_go_in_the_folder_of_their_day() {
        assert_eq!(
            day_of("rollout-2026-10-28T19-25-13-0199.jsonl", 0),
            Path::new("2026/10/28")
        );
        // 2026-10-28T12:00Z
        assert_eq!(
            day_of("thread.jsonl", 1_793_188_800_000),
            Path::new("2026/10/28")
        );
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(-1), (1969, 12, 31));
    }

    #[test]
    fn the_earlier_chat_reads_as_text() {
        let dir = tempfile::tempdir().unwrap();
        let chat = dir.path().join(CHAT);
        let lines = [
            json!({"seq": 0, "event": {"kind": "prompt", "text": "Which genes?"}}),
            json!({"seq": 1, "event": {"kind": "step", "tool": "Bash"}}),
            json!({"seq": 2, "event": {"kind": "text", "message": "m", "index": 1, "text": "and GAPDH."}}),
            json!({"seq": 3, "event": {"kind": "text", "message": "m", "index": 0, "text": "ACTB "}}),
        ];
        let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
        std::fs::write(&chat, text).unwrap();
        assert_eq!(
            earlier_text(&chat),
            "# Earlier in this conversation\n\n## The user\n\nWhich genes?\n\n## You\n\nACTB and GAPDH.\n"
        );
    }
}
