//! Account RPCs use a real daemon in a disposable home. CLI smoke tests run
//! installed, unmodified executables without signing in or making model calls.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use sandhill_daemon::accounts::Registry;
use sandhill_daemon::chats::Homes;
use sandhill_daemon::terminal::{Terminals, ToHost};
use sandhill_proto::client::Client;
use sandhill_proto::{AgentAccount, AgentOptions, Call, ChatEntry, TerminalInfo};
use serde_json::{Value, json};

const DAEMON: &str = env!("CARGO_BIN_EXE_sandhilld");
const TIMEOUT: Duration = Duration::from_secs(30);
const KEY: &str = "sandhill-integration-not-a-real-gemini-key";
const CLAUDE_LINKS: &[&str] = &[
    "projects",
    "settings.json",
    "CLAUDE.md",
    "agents",
    "commands",
    "skills",
    "plugins",
    "todos",
];
const CODEX_LINKS: &[&str] = &[
    "sessions",
    "archived_sessions",
    "config.toml",
    "AGENTS.md",
    "prompts",
    "skills",
];

struct Machine {
    dir: tempfile::TempDir,
    home: PathBuf,
    state: PathBuf,
    daemon: tokio::process::Child,
    client: Client,
}

impl Machine {
    async fn start(stand_in: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let state = dir.path().join("state");
        let bin = dir.path().join("bin");
        std::fs::create_dir(&home).unwrap();
        std::fs::create_dir(&bin).unwrap();
        if stand_in {
            // This is only a deterministic restart witness, never the real-CLI smoke test.
            let script = format!(
                "#!/bin/sh\nif [ \"$1\" = --help ]; then echo --resume; exit 0; fi\nprintf '%s\\n' \"$CLAUDE_CONFIG_DIR\" \"$@\" >> '{}'\nprintf '%s\\n' '{{\"session_id\":\"account-test-session\"}}' | '{}' terminal-event state idle\nwhile read -r line; do :; done\n",
                dir.path().join("launches").display(),
                DAEMON,
            );
            let program = bin.join("claude");
            std::fs::write(&program, script).unwrap();
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        // No inherited config, API keys, XDG directories or login-related env.
        let daemon = tokio::process::Command::new(DAEMON)
            .args(["--state-dir", state.to_str().unwrap(), "serve"])
            .env_clear()
            .env("HOME", &home)
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .env("SHELL", "/bin/sh")
            .env("TERM", "xterm-256color")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let client = connect(&state).await;
        Self {
            dir,
            home,
            state,
            daemon,
            client,
        }
    }

    fn root(&self) -> PathBuf {
        self.home.join(".local/share/sandhill/accounts")
    }

    fn folder(&self, account: &AgentAccount) -> PathBuf {
        let (kind, slug) = account.id.split_once(':').unwrap();
        self.root().join(kind).join(slug)
    }

    async fn add(&self, kind: &str, name: &str) -> AgentAccount {
        self.client
            .call(Call::AddAgentAccount {
                kind: kind.into(),
                name: name.into(),
                api_key: None,
            })
            .await
            .unwrap()
    }

    async fn finish(mut self) {
        for terminal in self.client.list_terminals().await.unwrap() {
            self.client.close_terminal(&terminal.id).await.unwrap();
        }
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

async fn connect(state: &Path) -> Client {
    tokio::time::timeout(TIMEOUT, async {
        loop {
            if let Ok(client) = Client::connect_unix(state.join("sandhilld.sock")).await {
                break client;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap()
}

fn seed_shared(home: &Path, kind: &str, links: &[&str]) -> BTreeMap<PathBuf, Vec<u8>> {
    let source = home.join(format!(".{kind}"));
    std::fs::create_dir_all(&source).unwrap();
    links
        .iter()
        .map(|name| {
            let target = source.join(name);
            let file = if name.contains('.') {
                target
            } else {
                std::fs::create_dir_all(&target).unwrap();
                target.join("sentinel")
            };
            let bytes = if *name == "config.toml" {
                b"cli_auth_credentials_store = \"file\"\n".to_vec()
            } else if *name == "settings.json" {
                b"{}\n".to_vec()
            } else {
                format!("shared {kind}/{name}\n").into_bytes()
            };
            std::fs::write(&file, &bytes).unwrap();
            (file, bytes)
        })
        .collect()
}

#[tokio::test]
async fn account_rpcs_preserve_shared_targets_and_default_logins() {
    let machine = Machine::start(false).await;
    let defaults: Vec<AgentAccount> = machine
        .client
        .call(Call::ListAgentAccounts { refresh: false })
        .await
        .unwrap();
    assert_eq!(defaults.len(), 3);
    for kind in ["claude", "codex", "antigravity"] {
        let account = defaults.iter().find(|account| account.id == kind).unwrap();
        assert!(account.default);
        assert_eq!(account.kind, kind);
        assert_eq!(account.name, "Default");
        assert_eq!(account.method, "login");
        assert!(
            machine
                .client
                .call::<Value>(Call::RemoveAgentAccount { id: kind.into() })
                .await
                .is_err()
        );
        assert!(
            machine
                .client
                .call::<Value>(Call::RenameAgentAccount {
                    id: kind.into(),
                    name: "Other".into()
                })
                .await
                .is_err()
        );
    }
    // Only the empty shared conversation directory may be created in the
    // original home; optional settings and credential paths remain absent.
    for (kind, required) in [("claude", "projects"), ("codex", "sessions")] {
        let account = machine.add(kind, "Empty home").await;
        let source = machine.home.join(format!(".{kind}"));
        assert!(source.join(required).is_dir());
        assert_eq!(std::fs::read_dir(&source).unwrap().count(), 1);
        assert_eq!(std::fs::read_dir(source.join(required)).unwrap().count(), 0);
        assert_eq!(
            std::fs::read_link(machine.folder(&account).join(required)).unwrap(),
            source.join(required)
        );
        let _: Value = machine
            .client
            .call(Call::RemoveAgentAccount { id: account.id })
            .await
            .unwrap();
    }
    for (kind, links, private) in [
        (
            "claude",
            CLAUDE_LINKS,
            &[".credentials.json", ".claude.json"][..],
        ),
        (
            "codex",
            CODEX_LINKS,
            &["auth.json", "state_5.sqlite", "state_5.sqlite-wal"][..],
        ),
    ] {
        let sentinels = seed_shared(&machine.home, kind, links);
        let account = machine.add(kind, "Lab account").await;
        assert_eq!(account.kind, kind);
        assert_eq!(account.name, "Lab account");
        assert_eq!(account.method, "login");
        assert!(!account.default);
        let folder = machine.folder(&account);
        assert!(folder.join("account.json").is_file());
        for name in links {
            let link = folder.join(name);
            assert!(
                std::fs::symlink_metadata(&link)
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
            assert_eq!(
                std::fs::read_link(link).unwrap(),
                machine.home.join(format!(".{kind}")).join(name)
            );
        }
        for name in private {
            assert!(
                std::fs::symlink_metadata(folder.join(name)).is_err(),
                "private state linked: {name}"
            );
        }
        let renamed: AgentAccount = machine
            .client
            .call(Call::RenameAgentAccount {
                id: account.id.clone(),
                name: "Renamed lab".into(),
            })
            .await
            .unwrap();
        assert_eq!(renamed.id, account.id);
        assert_eq!(renamed.name, "Renamed lab");
        assert_eq!(machine.folder(&renamed), folder);
        let listed: Vec<AgentAccount> = machine
            .client
            .call(Call::ListAgentAccounts { refresh: false })
            .await
            .unwrap();
        assert_eq!(
            listed.iter().find(|a| a.id == account.id).unwrap().name,
            "Renamed lab"
        );
        let _: Value = machine
            .client
            .call(Call::RemoveAgentAccount {
                id: account.id.clone(),
            })
            .await
            .unwrap();
        assert!(!folder.exists());
        for (file, bytes) in sentinels {
            assert_eq!(
                std::fs::read(file).unwrap(),
                bytes,
                "removal followed a shared symlink"
            );
        }
        let listed: Vec<AgentAccount> = machine
            .client
            .call(Call::ListAgentAccounts { refresh: false })
            .await
            .unwrap();
        assert!(!listed.iter().any(|a| a.id == account.id));
    }
    machine.finish().await;
}

fn registry(home: &Path) -> Registry {
    Registry::new(
        home.join(".local/share/sandhill/accounts"),
        Homes {
            claude: home.join(".claude"),
            codex: home.join(".codex"),
        },
    )
}

async fn version_with_env(program: &Path, home: &Path, env: &[(String, String)], args: &[&str]) {
    let mut command = tokio::process::Command::new(program);
    command
        .env_clear()
        .env("HOME", home)
        .env(
            "PATH",
            std::env::var_os("PATH").unwrap_or_else(|| OsString::from("/usr/bin:/bin")),
        )
        .envs(env.iter().map(|(key, value)| (key, value)))
        .args(args)
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let configured = command.as_std();
    assert!(
        !configured
            .get_args()
            .any(|arg| arg.to_string_lossy().contains(KEY)),
        "API key entered argv"
    );
    for (key, value) in env {
        assert!(
            configured
                .get_envs()
                .any(|(name, configured)| name == key.as_str()
                    && configured == Some(std::ffi::OsStr::new(value)))
        );
    }
    let output = tokio::time::timeout(TIMEOUT, command.output())
        .await
        .unwrap()
        .unwrap();
    assert!(
        output.status.success(),
        "{} --version failed: {}",
        program.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.stdout.is_empty(), "CLI returned no version");
    assert!(!String::from_utf8_lossy(&output.stdout).contains(KEY));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(KEY));
}

#[tokio::test]
async fn installed_clis_start_with_isolated_account_environments_without_authentication() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir(&home).unwrap();
    let registry = registry(&home);
    for (kind, variable) in [("claude", "CLAUDE_CONFIG_DIR"), ("codex", "CODEX_HOME")] {
        let Some(program) = sandhill_daemon::agents::find(kind) else {
            eprintln!("skipping {kind} account launch: CLI is not installed");
            continue;
        };
        let account = registry
            .add(kind, "No authentication smoke test", None)
            .unwrap();
        let env = registry.environment(kind, Some(&account.id)).unwrap();
        let value = &env.iter().find(|(name, _)| name == variable).unwrap().1;
        assert!(Path::new(value).starts_with(&home));
        assert_eq!(
            env.iter()
                .find(|(name, _)| name == "SANDHILL_ACCOUNT_ID")
                .unwrap()
                .1,
            account.id
        );
        let args = if kind == "codex" {
            &["-c", "cli_auth_credentials_store=\"file\"", "--version"][..]
        } else {
            &["--version"][..]
        };
        version_with_env(&program, &home, &env, args).await;
        assert!(
            registry
                .environment(
                    if kind == "codex" { "claude" } else { "codex" },
                    Some(&account.id)
                )
                .is_err()
        );
        registry.remove(&account.id, &[]).unwrap();
    }
    let Some(agy) = sandhill_daemon::agents::find("agy") else {
        eprintln!("skipping Antigravity API-key launch: agy is not installed");
        return;
    };
    #[cfg(target_os = "macos")]
    let (env, account_id) = {
        eprintln!(
            "Antigravity smoke uses direct launch env on macOS; no real Keychain entry is read or written"
        );
        (
            vec![
                ("GEMINI_API_KEY".into(), KEY.into()),
                ("SANDHILL_AGY_API_KEY".into(), "1".into()),
            ],
            None::<String>,
        )
    };
    #[cfg(not(target_os = "macos"))]
    let (env, account_id) = {
        let account = registry
            .add("antigravity", "Disposable API key", Some(KEY))
            .unwrap();
        let env = registry
            .environment("antigravity", Some(&account.id))
            .unwrap();
        let serialized = serde_json::to_string(&account).unwrap();
        assert!(!serialized.contains(KEY));
        (env, Some(account.id))
    };
    assert_eq!(
        env.iter()
            .find(|(name, _)| name == "GEMINI_API_KEY")
            .unwrap()
            .1,
        KEY
    );
    version_with_env(&agy, &home, &env, &["--version"]).await;
    if let Some(id) = account_id {
        registry.remove(&id, &[]).unwrap();
    }
}

async fn ready(machine: &Machine, terminal: &str) -> TerminalInfo {
    tokio::time::timeout(TIMEOUT, async {
        loop {
            let info = machine
                .client
                .list_terminals()
                .await
                .unwrap()
                .into_iter()
                .find(|t| t.id == terminal)
                .unwrap();
            assert!(info.running, "stand-in exited before reporting its session");
            if info
                .agent
                .as_ref()
                .is_some_and(|a| a["session_id"] == "account-test-session")
            {
                break info;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap()
}

async fn chat(state: &Path, terminal: &str) -> Vec<ChatEntry> {
    let mut client = connect(state).await;
    client.subscribe_chat(terminal, Some(0), 0).await.unwrap();
    tokio::time::timeout(TIMEOUT, async {
        let mut entries = Vec::new();
        while let Some(event) = client.next_terminal_event().await {
            if let Some(entry) = event.chat {
                entries.push(entry);
            }
            if event.exit.is_some() {
                return entries;
            }
        }
        panic!("chat stream disconnected without terminal exit");
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn switching_accounts_restarts_the_same_chat_and_replays_one_divider() {
    let machine = Machine::start(true).await;
    let first = machine.add("claude", "First").await;
    let second = machine.add("claude", "Second").await;
    let terminal: TerminalInfo = machine
        .client
        .call(Call::StartTerminal {
            kind: "claude".into(),
            cwd: Some(machine.home.to_string_lossy().into_owned()),
            project: None,
            cols: None,
            rows: None,
            prompt: None,
            options: AgentOptions {
                account: Some(first.id.clone()),
                ..AgentOptions::default()
            },
        })
        .await
        .unwrap();
    ready(&machine, &terminal.id).await;
    let terminals = Terminals::new(machine.state.join("terminals"), DAEMON.into());
    terminals.send(&terminal.id, ToHost::Chat {
        event: json!({"kind":"text", "text":"context before the account switch", "at_ms":1}),
    }).await.unwrap();
    assert!(
        machine
            .client
            .call::<Value>(Call::RemoveAgentAccount {
                id: first.id.clone()
            })
            .await
            .is_err()
    );
    let switched: TerminalInfo = machine
        .client
        .call(Call::RestartTerminal {
            terminal: terminal.id.clone(),
            options: Some(AgentOptions {
                account: Some(second.id.clone()),
                ..AgentOptions::default()
            }),
            cols: None,
            rows: None,
            prompt: None,
        })
        .await
        .unwrap();
    assert_eq!(switched.id, terminal.id);
    assert_eq!(switched.created_ms, terminal.created_ms);
    assert_eq!(
        switched.options.account.as_deref(),
        Some(second.id.as_str())
    );
    assert!(switched.started_ms >= terminal.started_ms);
    ready(&machine, &terminal.id).await;
    // Restarting the same account must not manufacture another switch.
    let _: TerminalInfo = machine
        .client
        .call(Call::RestartTerminal {
            terminal: terminal.id.clone(),
            options: None,
            cols: None,
            rows: None,
            prompt: None,
        })
        .await
        .unwrap();
    ready(&machine, &terminal.id).await;
    machine.client.stop_terminal(&terminal.id).await.unwrap();
    tokio::time::timeout(TIMEOUT, async {
        while terminals.info(&terminal.id).unwrap().running {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let entries = chat(&machine.state, &terminal.id).await;
    assert!(
        entries
            .iter()
            .any(|entry| entry.event["text"] == "context before the account switch")
    );
    assert!(entries.windows(2).all(|pair| pair[0].seq < pair[1].seq));
    let dividers: Vec<_> = entries
        .iter()
        .filter(|entry| entry.event["kind"] == "account")
        .collect();
    assert_eq!(dividers.len(), 1, "{entries:#?}");
    assert_eq!(dividers[0].event["account"], second.id);
    assert_eq!(dividers[0].event["name"], "Second");
    assert!(dividers[0].event["at_ms"].as_i64().is_some());
    let launches = std::fs::read_to_string(machine.dir.path().join("launches")).unwrap();
    assert_eq!(
        launches
            .matches(machine.folder(&first).to_str().unwrap())
            .count(),
        1
    );
    assert_eq!(
        launches
            .matches(machine.folder(&second).to_str().unwrap())
            .count(),
        2
    );
    assert_eq!(
        launches.matches("--resume\naccount-test-session\n").count(),
        2
    );
    // A fresh daemon must replay the divider from disk, not synthesize it
    // from the latest account option or lose the earlier conversation.
    let _: Value = machine.client.call(Call::StopDaemon).await.unwrap();
    let mut machine = machine;
    assert!(
        tokio::time::timeout(TIMEOUT, machine.daemon.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    machine.daemon = tokio::process::Command::new(DAEMON)
        .args(["--state-dir", machine.state.to_str().unwrap(), "serve"])
        .env_clear()
        .env("HOME", &machine.home)
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    machine.client = connect(&machine.state).await;
    let replay = chat(&machine.state, &terminal.id).await;
    assert_eq!(replay, entries);
    let _: Value = machine
        .client
        .call(Call::RemoveAgentAccount { id: first.id })
        .await
        .unwrap();
    let _: Value = machine
        .client
        .call(Call::RemoveAgentAccount { id: second.id })
        .await
        .unwrap();
    machine.finish().await;
}

#[tokio::test]
async fn sign_in_runs_in_an_account_scoped_non_agent_terminal() {
    let machine = Machine::start(true).await;
    let account = machine.add("claude", "First-run sign-in").await;
    let login: TerminalInfo = machine
        .client
        .call(Call::SignInAgentAccount {
            id: account.id.clone(),
            cols: Some(80),
            rows: Some(24),
        })
        .await
        .unwrap();
    let login = ready(&machine, &login.id).await;
    assert_eq!(login.kind, sandhill_proto::TERMINAL_LOGIN);
    assert!(!sandhill_daemon::agents::is_agent(&login.kind));
    assert_eq!(login.options.account.as_deref(), Some(account.id.as_str()));
    assert!(
        machine
            .client
            .call::<Value>(Call::RemoveAgentAccount {
                id: account.id.clone()
            })
            .await
            .is_err()
    );
    let launches = std::fs::read_to_string(machine.dir.path().join("launches")).unwrap();
    assert!(launches.starts_with(machine.folder(&account).to_str().unwrap()));
    // An interactive first-run has no auth-login-only arguments: onboarding
    // belongs to the real CLI, rather than metadata manufactured by Sandhill.
    assert!(!launches.contains("auth\nlogin"));
    machine.finish().await;
}

/// A banked reset is spent only through Codex's own app-server, in the
/// account's own home, naming the credit asked for, with a fresh idempotency
/// key; the account is read again afterwards. A stand-in Codex keeps real
/// resets untouched.
#[tokio::test]
async fn using_a_banked_reset_asks_codex_in_the_accounts_home() {
    let machine = Machine::start(false).await;
    let log = machine.dir.path().join("consumed");
    let script = format!(
        "#!/bin/sh\nwhile IFS= read -r line; do\n  case \"$line\" in\n    *'\"method\":\"initialize\"'*) echo '{{\"id\":1,\"result\":{{}}}}' ;;\n    *'rateLimitResetCredit/consume'*) printf '%s|%s\\n' \"$CODEX_HOME\" \"$line\" >> '{}'; echo '{{\"id\":2,\"result\":{{\"outcome\":\"reset\"}}}}' ;;\n    *'\"method\":\"account/read\"'*) echo '{{\"id\":2,\"result\":{{\"account\":{{\"type\":\"chatgpt\",\"email\":\"spare@example.org\",\"planType\":\"plus\"}}}}}}' ;;\n    *'\"method\":\"account/rateLimits/read\"'*) echo '{{\"id\":3,\"result\":{{\"rateLimits\":{{\"primary\":{{\"usedPercent\":0,\"resetsAt\":2000000000,\"windowDurationMins\":300}}}},\"rateLimitResetCredits\":{{\"availableCount\":1,\"credits\":[]}}}}}}' ;;\n  esac\ndone\n",
        log.display()
    );
    let program = machine.dir.path().join("bin/codex");
    std::fs::write(&program, script).unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
    let account = machine.add("codex", "Spare").await;
    let used: Value = machine
        .client
        .call(Call::UseAgentReset {
            id: account.id.clone(),
            credit: Some("credit-7".into()),
        })
        .await
        .unwrap();
    assert_eq!(used["outcome"], "reset");
    assert_eq!(used["account"]["usage"]["reset_credits"]["available"], 1);
    let consumed = std::fs::read_to_string(&log).unwrap();
    let (home, request) = consumed.trim_end().split_once('|').unwrap();
    assert_eq!(home, machine.folder(&account).to_str().unwrap());
    let request: Value = serde_json::from_str(request).unwrap();
    assert_eq!(request["params"]["creditId"], "credit-7");
    assert!(
        request["params"]["idempotencyKey"]
            .as_str()
            .is_some_and(|key| !key.is_empty())
    );
    // Only Codex reports resets Sandhill can use.
    let claude = machine.add("claude", "Other").await;
    assert!(
        machine
            .client
            .call::<Value>(Call::UseAgentReset {
                id: claude.id,
                credit: None,
            })
            .await
            .is_err()
    );
    machine.finish().await;
}
