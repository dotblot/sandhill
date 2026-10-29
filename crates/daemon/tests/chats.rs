//! Conversations kept with their project: one carried to another machine
//! on the project's folder shows in its history there, and its agent goes
//! on with it, or starts afresh with the chat to read. The machines are two
//! state folders and two homes; no agent runs.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use sandhill_daemon::chats::{self, Homes};
use sandhill_daemon::terminal::{Adopted, Terminals};
use sandhill_daemon::{Daemon, StateDir};
use sandhill_proto::client::Client;
use sandhill_proto::{
    AgentOptions, Call, ChatHistory, ProjectInfo, ProjectSettings, ProjectSetup, TerminalInfo,
};
use serde_json::json;
use tokio::net::UnixListener;

const ID: &str = "0a1b2c3d4e5f";

struct Machine {
    name: &'static str,
    terminals: Terminals,
    homes: Homes,
}

impl Machine {
    fn new(dir: &Path, name: &'static str) -> Machine {
        let home = dir.join(name);
        Machine {
            name,
            terminals: Terminals::new(
                home.join("terminals"),
                env!("CARGO_BIN_EXE_sandhilld").into(),
            ),
            homes: Homes {
                claude: home.join(".claude"),
                codex: home.join(".codex"),
            },
        }
    }

    fn sync(&self) {
        chats::sync(&self.terminals, &self.homes, self.name);
    }

    fn conversation(&self) -> TerminalInfo {
        self.terminals.info(ID).unwrap()
    }

    fn chat(&self) -> String {
        std::fs::read_to_string(self.terminals.dir(ID).unwrap().join("chat.jsonl")).unwrap()
    }

    /// Claude Code's own copy of the conversation, for its folder here.
    fn transcript(&self, project: &Path) -> PathBuf {
        chats::claude_transcript(&self.homes, project, "s1")
    }
}

fn line(seq: u64, kind: &str, text: &str) -> String {
    format!(
        "{}\n",
        json!({"seq": seq, "event": {"kind": kind, "message": "m", "index": 0, "text": text}})
    )
}

/// Copies a folder, as a drive or a sync service would.
fn copy(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// Machine A had a conversation with Claude Code in `project`.
fn conversation_on(a: &Machine, project: &Path, dir: &Path) {
    let chat = dir.join("chat.jsonl");
    std::fs::write(
        &chat,
        line(0, "prompt", "Which genes?") + &line(1, "text", "ACTB."),
    )
    .unwrap();
    let mut report = serde_json::Map::new();
    report.insert("session_id".into(), json!("s1"));
    a.terminals
        .adopt(
            Adopted {
                id: ID.into(),
                kind: "claude".into(),
                title: "Claude Code".into(),
                name: Some("Marker genes".into()),
                project: project.to_string_lossy().into_owned(),
                options: AgentOptions::default(),
                created_ms: 1,
                ended_ms: 2,
                report,
            },
            &chat,
        )
        .unwrap();
    let transcript = a.transcript(project);
    std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    std::fs::write(&transcript, "{\"turn\":1}\n").unwrap();
}

#[test]
fn a_conversation_goes_on_on_another_machine() {
    let dir = tempfile::tempdir().unwrap();
    let a = Machine::new(dir.path(), "mac-a");
    let b = Machine::new(dir.path(), "mac-b");
    let on_a = dir.path().join("Users/huan/rnaseq");
    let on_b = dir.path().join("Volumes/Lab Drive/rnaseq");
    std::fs::create_dir_all(&on_a).unwrap();
    conversation_on(&a, &on_a, dir.path());

    // A project keeps its chats by default, out of git.
    a.sync();
    let kept = on_a.join(chats::DIR).join(ID);
    for file in ["chat.jsonl", "conversation.json", "session.jsonl"] {
        assert!(kept.join(file).is_file(), "no {file}");
    }
    let ignore = std::fs::read_to_string(on_a.join(".sandhill/.gitignore")).unwrap();
    assert!(ignore.lines().any(|l| l == "agents/"), "{ignore}");

    // On machine B, at another path, it shows in the history and Claude
    // Code finds its own copy where it looks for this path.
    copy(&on_a, &on_b);
    chats::adopt(&b.terminals, &on_b);
    let there = b.conversation();
    assert_eq!(there.project.as_deref(), Some(on_b.to_str().unwrap()));
    assert_eq!(there.cwd, on_b.to_str().unwrap());
    assert_eq!(there.name.as_deref(), Some("Marker genes"));
    assert!(!there.running);
    assert_eq!(b.chat(), a.chat());
    let resume = chats::prepare(&there, &b.homes, b.name).unwrap();
    assert_eq!(resume.id.as_deref(), Some("s1"));
    assert_eq!(resume.note, None);
    assert_eq!(
        std::fs::read_to_string(b.transcript(&on_b)).unwrap(),
        "{\"turn\":1}\n"
    );

    // It goes on there, and the news comes back with the folder.
    let local = b.terminals.dir(ID).unwrap().join("chat.jsonl");
    let more = line(2, "prompt", "And in B cells?") + &line(3, "text", "CD19.");
    std::fs::write(&local, b.chat() + &more).unwrap();
    std::fs::write(b.transcript(&on_b), "{\"turn\":1}\n{\"turn\":2}\n").unwrap();
    b.sync();
    std::fs::remove_dir_all(on_a.join(chats::DIR)).unwrap();
    copy(&on_b.join(chats::DIR), &on_a.join(chats::DIR));
    a.sync();
    assert_eq!(a.chat(), b.chat());
    let kept: serde_json::Value =
        serde_json::from_slice(&std::fs::read(kept.join("conversation.json")).unwrap()).unwrap();
    assert_eq!(kept["machine"], "mac-b");
    assert_eq!(
        std::fs::read_to_string(on_a.join(chats::DIR).join(ID).join("session.jsonl")).unwrap(),
        "{\"turn\":1}\n{\"turn\":2}\n"
    );
    // A's Claude Code gets the newer copy before going on.
    chats::prepare(&a.conversation(), &a.homes, a.name).unwrap();
    assert_eq!(
        std::fs::read_to_string(a.transcript(&on_a)).unwrap(),
        "{\"turn\":1}\n{\"turn\":2}\n"
    );
}

#[test]
fn an_agent_without_its_copy_starts_afresh_with_the_chat() {
    let dir = tempfile::tempdir().unwrap();
    let a = Machine::new(dir.path(), "mac-a");
    let c = Machine::new(dir.path(), "mac-c");
    let project = dir.path().join("rnaseq");
    std::fs::create_dir_all(&project).unwrap();
    conversation_on(&a, &project, dir.path());
    a.sync();
    let kept = project.join(chats::DIR).join(ID);
    std::fs::remove_file(kept.join("session.jsonl")).unwrap();

    std::fs::remove_dir_all(a.terminals.dir(ID).unwrap()).unwrap();
    chats::adopt(&c.terminals, &project);
    let resume = chats::prepare(&c.conversation(), &c.homes, c.name).unwrap();
    assert_eq!(resume.id, None);
    let note = resume.note.unwrap();
    assert!(
        note.contains(&kept.join("earlier.md").display().to_string()),
        "{note}"
    );
    let earlier = std::fs::read_to_string(kept.join("earlier.md")).unwrap();
    assert!(
        earlier.contains("Which genes?") && earlier.contains("ACTB."),
        "{earlier}"
    );
    assert!(chats::with_note(&note, Some("Go on.")).ends_with("\n\nGo on."));

    // Not while it runs on another machine.
    let path = kept.join("conversation.json");
    let mut conversation: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    conversation["machine"] = json!("mac-z");
    conversation["running_ms"] = json!(sandhill_daemon::journal::now_ms());
    std::fs::write(&path, conversation.to_string()).unwrap();
    let err = chats::prepare(&c.conversation(), &c.homes, c.name).unwrap_err();
    assert!(err.to_string().contains("running on mac-z"), "{err}");
}

#[test]
fn a_conversation_removed_on_one_machine_goes_from_the_others() {
    let dir = tempfile::tempdir().unwrap();
    let a = Machine::new(dir.path(), "mac-a");
    let b = Machine::new(dir.path(), "mac-b");
    let project = dir.path().join("Dropbox/rnaseq");
    std::fs::create_dir_all(&project).unwrap();
    conversation_on(&a, &project, dir.path());
    a.sync();
    chats::adopt(&b.terminals, &project);
    assert!(!b.conversation().running);

    // Removed on A: only a note that it was removed stays in the folder.
    std::fs::remove_dir_all(a.terminals.dir(ID).unwrap()).unwrap();
    chats::forget(&project, ID, a.name).unwrap();
    let kept = project.join(chats::DIR).join(ID);
    let names: Vec<_> = std::fs::read_dir(&kept)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, ["conversation.json"]);
    let note = std::fs::read_to_string(kept.join("conversation.json")).unwrap();
    assert!(
        note.contains("removed_ms") && !note.contains("Marker genes"),
        "{note}"
    );

    // B removes its copy instead of putting the chat back, and a machine
    // opening the project later doesn't bring it back either.
    b.sync();
    assert!(b.terminals.info(ID).is_err());
    assert!(!kept.join("chat.jsonl").exists());
    let c = Machine::new(dir.path(), "mac-c");
    chats::adopt(&c.terminals, &project);
    assert!(c.terminals.list().is_empty());
}

/// Through the daemon: opening a project brings its conversations into
/// the history, and keeping them on the machine moves them out of the
/// folder.
#[tokio::test]
async fn opening_a_project_shows_its_conversations() {
    let dir = tempfile::tempdir().unwrap();
    let a = Machine::new(dir.path(), "mac-a");
    let on_a = dir.path().join("a/rnaseq");
    let project = dir.path().join("b/rnaseq");
    std::fs::create_dir_all(&on_a).unwrap();
    conversation_on(&a, &on_a, dir.path());
    a.sync();
    copy(&on_a, &project);

    let state = dir.path().join("state");
    let daemon: Arc<Daemon> = Daemon::open(StateDir::new(&state)).await.unwrap();
    let socket = state.join("test.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    tokio::spawn({
        let daemon = daemon.clone();
        async move {
            let _ = daemon.serve(listener).await;
        }
    });
    let client = Client::connect_unix(&socket).await.unwrap();
    let path = project.to_string_lossy().into_owned();
    let opened: ProjectInfo = client
        .call(Call::OpenProject { path: path.clone() })
        .await
        .unwrap();
    assert_eq!(opened.path, path);
    let listed: Vec<TerminalInfo> = client.call(Call::ListTerminals).await.unwrap();
    let conversation = listed.iter().find(|t| t.id == ID).expect("not listed");
    assert_eq!(conversation.project.as_deref(), Some(path.as_str()));

    let settings = ProjectSettings {
        chat_history: ChatHistory::Machine,
        ..ProjectSettings::default()
    };
    let setup: ProjectSetup = client
        .call(Call::SaveProjectSettings {
            path: path.clone(),
            settings,
        })
        .await
        .unwrap();
    assert_eq!(setup.settings.chat_history, ChatHistory::Machine);
    assert!(!project.join(chats::DIR).exists());
    let listed: Vec<TerminalInfo> = client.call(Call::ListTerminals).await.unwrap();
    assert!(listed.iter().any(|t| t.id == ID));
    assert!(!chats::kept_with(&project));
}
