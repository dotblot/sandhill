//! Real request dispatch and metadata scans, with no access to the user's home.
mod common;

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use sandhill_proto::Call;
use sandhill_proto::client::Client;
use serde_json::{Value, json};

const TIMEOUT: Duration = Duration::from_secs(30);

struct Machine {
    dir: common::TempDir,
    home: PathBuf,
    project: PathBuf,
    daemon: tokio::process::Child,
    client: Client,
}

impl Machine {
    async fn start(program: Option<&Path>) -> Self {
        let dir = common::tempdir().unwrap();
        let home = dir.path().join("home");
        let project = dir.path().join("project");
        let state = dir.path().join("state");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&project).unwrap();
        let mut paths: Vec<PathBuf> = program
            .and_then(Path::parent)
            .into_iter()
            .map(Path::to_path_buf)
            .collect();
        paths.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        let daemon = tokio::process::Command::new(env!("CARGO_BIN_EXE_sandhilld"))
            .args(["--state-dir", state.to_str().unwrap(), "serve"])
            .env_clear()
            .env("HOME", &home)
            .env("PATH", std::env::join_paths(paths).unwrap())
            .env("SHELL", "/bin/sh")
            .env("SANDHILL_CODEX_FILE_AUTH", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let client = tokio::time::timeout(TIMEOUT, async {
            loop {
                if let Ok(client) = Client::connect_unix(state.join("sandhilld.sock")).await {
                    break client;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        Self {
            dir,
            home,
            project,
            daemon,
            client,
        }
    }

    async fn list(&self, kind: &str) -> Vec<Value> {
        self.client
            .agent_commands(kind, self.project.to_str().unwrap())
            .await
            .unwrap()
    }

    async fn finish(mut self) {
        let _: Value = self.client.call(Call::StopDaemon).await.unwrap();
        assert!(
            tokio::time::timeout(TIMEOUT, self.daemon.wait())
                .await
                .unwrap()
                .unwrap()
                .success()
        );
    }
}

fn put(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn assert_guides(items: &[Value]) {
    let expected: Vec<_> = sandhill_daemon::bio::guides::titles().collect();
    assert!(items.len() >= expected.len());
    for (item, (id, title)) in items[items.len() - expected.len()..].iter().zip(expected) {
        assert_eq!(item["kind"], "guide");
        assert_eq!(item["name"], id);
        assert_eq!(item["description"], title);
        assert!(item["prompt"].as_str().unwrap().contains("get_bio_guide"));
        assert_eq!(item.as_object().unwrap().len(), 4);
    }
    for item in items {
        assert!(!item["name"].as_str().unwrap().starts_with('/'));
        assert!(!item["description"].as_str().unwrap().contains('\n'));
    }
}

#[tokio::test]
async fn agent_commands_scans_project_and_user_metadata_then_appends_guides() {
    let machine = Machine::start(None).await;
    put(
        &machine.home.join(".claude/commands/team/review.md"),
        "User review",
    );
    put(
        &machine.project.join(".claude/commands/team/review.md"),
        "---\ndescription: 'Project review: concise'\n---\nPRIVATE BODY",
    );
    put(
        &machine.home.join(".claude/commands/user.md"),
        "\n User command\n",
    );
    put(
        &machine.project.join(".claude/skills/old/SKILL.md"),
        "---\nname: analysis\ndescription: >-\n  Inspect the study\n  before analysis.\n---\nPRIVATE BODY",
    );
    put(
        &machine.home.join(".claude/skills/old/SKILL.md"),
        "---\nname: analysis\ndescription: user version\n---\n",
    );
    let items = machine.list("claude").await;
    assert_guides(&items);
    assert!(items.contains(
        &json!({"kind": "command", "name": "team:review", "description": "Project review: concise"})
    ));
    assert!(
        items.contains(&json!({"kind": "command", "name": "user", "description": "User command"}))
    );
    assert!(items.contains(&json!({"kind": "skill", "name": "analysis", "description": "Inspect the study before analysis."})));
    assert_eq!(items.iter().filter(|i| i["name"] == "analysis").count(), 1);
    for builtin in ["init", "review", "security-review"] {
        assert!(
            items
                .iter()
                .any(|i| i["kind"] == "command" && i["name"] == builtin)
        );
    }
    assert!(
        !serde_json::to_string(&items)
            .unwrap()
            .contains("PRIVATE BODY")
    );

    put(
        &machine.home.join(".gemini/config/skills/global/SKILL.md"),
        "---\nname: agy-global\ndescription: Global guide\n---\n",
    );
    put(
        &machine.project.join(".agents/skills/local/SKILL.md"),
        "---\nname: agy-local\ndescription: Local guide\n---\n",
    );
    put(
        &machine.home.join(".codex/skills/unsupported/SKILL.md"),
        "---\nname: not-an-agy-skill\n---\n",
    );
    let items = machine.list("antigravity").await;
    assert_guides(&items);
    assert_eq!(items.len(), 6);
    assert!(items.iter().any(|i| i["name"] == "agy-global"));
    assert!(items.iter().any(|i| i["name"] == "agy-local"));
    for kind in ["openscience", "omp", "posit", "unknown"] {
        let items = machine.list(kind).await;
        assert_guides(&items);
        assert_eq!(items.len(), 4);
    }
    // Process startup failures are a successful guides-only response.
    for kind in ["codex", "pi"] {
        let items: Vec<Value> = machine
            .client
            .agent_commands(kind, machine.dir.path().join("missing").to_str().unwrap())
            .await
            .unwrap();
        assert_guides(&items);
        assert_eq!(items.len(), 4);
    }
    machine.finish().await;
}

async fn installed_skills(kind: &str, folder: &str, name: &str) {
    let Some(program) = sandhill_daemon::agents::find(kind) else {
        eprintln!("skipping: {kind} is not installed");
        return;
    };
    let machine = Machine::start(Some(&program)).await;
    // pi intentionally ignores untrusted project customizations. Listing must
    // not grant project trust, so its fixture uses the disposable user's skills.
    let root = if kind == "pi" {
        &machine.home
    } else {
        &machine.project
    };
    let path = root.join(folder).join("sandhill-test/SKILL.md");
    put(
        &path,
        "---\nname: sandhill-test\ndescription: Test metadata discovery without a model call\n---\nOnly used to test metadata discovery.\n",
    );
    let items = machine.list(kind).await;
    assert_guides(&items);
    let skill = items
        .iter()
        .find(|item| item["name"] == name)
        .unwrap_or_else(|| {
            panic!("installed {kind} did not return the fixture's skill: {items:#?}")
        });
    assert_eq!(skill["kind"], "skill");
    assert_eq!(
        skill["path"]
            .as_str()
            .map(|p| std::fs::canonicalize(p).unwrap()),
        Some(path.canonicalize().unwrap())
    );
    assert_eq!(
        skill["description"],
        "Test metadata discovery without a model call"
    );
    machine.finish().await;
}

#[tokio::test]
async fn installed_codex_lists_project_skills_without_a_conversation() {
    installed_skills("codex", ".agents/skills", "sandhill-test").await;
}

#[tokio::test]
async fn installed_pi_lists_user_skills_without_saving_a_session() {
    installed_skills("pi", ".pi/agent/skills", "skill:sandhill-test").await;
}
