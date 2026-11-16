//! Device-local skill installation and real socket/MCP discovery, without kernels.
mod common;

use std::fs;
use std::io::{Cursor, Write};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use sandhill_daemon::mcp::Tools;
use sandhill_proto::client::Client;
use sandhill_proto::{
    Call, SkillExport, SkillFetch, SkillFile, SkillInfo, SkillList, SkillPreview, SkillSource,
};
use serde_json::{Value, json};

struct Machine {
    _dir: common::TempDir,
    state: PathBuf,
    child: tokio::process::Child,
    client: Client,
}

impl Machine {
    async fn start() -> Self {
        let dir = common::tempdir().unwrap();
        let state = dir.path().join("state");
        let home = dir.path().join("home");
        fs::create_dir(&home).unwrap();
        let child = tokio::process::Command::new(env!("CARGO_BIN_EXE_sandhilld"))
            .args(["--state-dir", state.to_str().unwrap(), "serve"])
            .env_clear()
            .env("HOME", home)
            .env("SHELL", "/bin/sh")
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let client = tokio::time::timeout(Duration::from_secs(30), async {
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
            _dir: dir,
            state,
            child,
            client,
        }
    }

    async fn preview(&self, fetch: SkillFetch) -> SkillPreview {
        self.client
            .call(Call::PreviewSkills { fetch })
            .await
            .unwrap()
    }

    async fn install(&self, preview: &SkillPreview) -> Vec<SkillInfo> {
        self.client
            .call(Call::InstallSkills {
                token: preview.token.clone(),
                names: preview.skills.iter().map(|s| s.name.clone()).collect(),
            })
            .await
            .unwrap()
    }

    async fn list(&self) -> SkillList {
        self.client.call(Call::ListSkills).await.unwrap()
    }

    async fn get(&self, name: Option<&str>, file: Option<&str>) -> Value {
        self.client
            .call(Call::GetSkill {
                name: name.map(str::to_owned),
                file: file.map(str::to_owned),
            })
            .await
            .unwrap()
    }

    async fn finish(mut self) {
        let _: Value = self.client.call(Call::StopDaemon).await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(30), self.child.wait())
                .await
                .unwrap()
                .unwrap()
                .success()
        );
    }
}

fn file(path: &str, bytes: &[u8], executable: bool) -> SkillFile {
    SkillFile {
        path: path.into(),
        base64: BASE64.encode(bytes),
        executable,
    }
}

fn skill(name: &str) -> Vec<SkillFile> {
    vec![file("SKILL.md", format!("---\nname: {name}\ndescription: Read and analyse sequencing data.\n---\n# Instructions\nRead the reference first.\n").as_bytes(), false),
        file("references/notes.txt", b"a reference\n", false),
        file("scripts/run", &[0, 1, 255, 10], true)]
}

fn upload(files: Vec<SkillFile>) -> SkillFetch {
    SkillFetch::Files {
        label: "test folder".into(),
        files,
        source: None,
    }
}

fn assert_files(folder: &Path, files: &[SkillFile]) {
    for file in files {
        let path = folder.join(&file.path);
        assert_eq!(
            fs::read(&path).unwrap(),
            BASE64.decode(&file.base64).unwrap()
        );
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o111 != 0,
            file.executable
        );
    }
}

fn zip_files(files: &[SkillFile]) -> String {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for file in files {
        zip.start_file(
            &file.path,
            zip::write::SimpleFileOptions::default().unix_permissions(if file.executable {
                0o755
            } else {
                0o644
            }),
        )
        .unwrap();
        zip.write_all(&BASE64.decode(&file.base64).unwrap())
            .unwrap();
    }
    BASE64.encode(zip.finish().unwrap().into_inner())
}

#[tokio::test]
async fn files_install_replace_toggle_remove_and_mcp() {
    let machine = Machine::start().await;
    let files = skill("..My Skill.. ");
    let preview = machine.preview(upload(files.clone())).await;
    assert!(preview.refused.is_empty());
    assert_eq!(preview.skills[0].name, "my-skill");
    assert!(!preview.skills[0].replaces);
    assert!(preview.skills[0].changed);
    assert_eq!(preview.skills[0].scripts, ["scripts/run"]);
    let installed = machine.install(&preview).await;
    let folder = machine.state.join("skills/my-skill");
    assert_files(&folder, &files);
    assert_eq!(installed[0].files, 3);
    assert_eq!(installed[0].path, folder.to_str().unwrap());
    assert!(
        !machine
            .state
            .join("skills/.incoming")
            .join(&preview.token)
            .exists()
    );
    let listed = machine.list().await;
    assert_eq!(listed.installed, installed);
    assert_eq!(listed.built_in.len(), 4);
    assert_eq!(listed.suggested.len(), 8);
    assert!(listed.suggested.iter().all(|s| !s.installed));

    let tools = Tools::new(machine.state.join("sandhilld.sock"), None);
    assert!(
        sandhill_daemon::mcp::read_only_tools()
            .iter()
            .any(|s| s == "get_skill")
    );
    let items = tools
        .call("get_skill", &json!({"name": "my-skill"}))
        .await
        .unwrap();
    let value: Value = serde_json::from_str(items[0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(value["path"], folder.to_str().unwrap());
    assert!(value["text"].as_str().unwrap().contains("# Instructions"));
    assert_eq!(
        value["files"],
        json!(["SKILL.md", "references/notes.txt", "scripts/run"])
    );
    let items = tools
        .call(
            "get_skill",
            &json!({"name": "my-skill", "file": "references/notes.txt"}),
        )
        .await
        .unwrap();
    let value: Value = serde_json::from_str(items[0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(value["text"], "a reference\n");
    assert!(
        tools
            .call(
                "get_skill",
                &json!({"name": "my-skill", "file": "../index.json"})
            )
            .await
            .is_err()
    );
    assert!(
        tools
            .call(
                "get_skill",
                &json!({"name": "my-skill", "file": "scripts/run"})
            )
            .await
            .is_err()
    );

    for enabled in [false, true] {
        let _: Value = machine
            .client
            .call(Call::SetSkillEnabled {
                name: "my-skill".into(),
                enabled,
            })
            .await
            .unwrap();
        assert_eq!(
            machine.get(None, None).await["skills"]
                .as_array()
                .unwrap()
                .len(),
            usize::from(enabled)
        );
        let commands: Vec<Value> = machine
            .client
            .call(Call::AgentCommands {
                kind: "unknown".into(),
                cwd: machine.state.to_string_lossy().into_owned(),
            })
            .await
            .unwrap();
        assert_eq!(commands.len(), 4 + usize::from(enabled));
        if enabled {
            assert_eq!(commands[4]["name"], "my-skill");
            assert_eq!(commands[4]["kind"], "guide");
            assert_eq!(
                commands[4]["prompt"],
                "For this message, follow the skill “my-skill”: read it first with the get_skill tool."
            );
        } else {
            let error = tools
                .call("get_skill", &json!({"name": "my-skill"}))
                .await
                .unwrap_err()
                .to_string();
            assert!(error.contains("no installed skill named my-skill (get_skill lists them)"));
        }
    }
    let unchanged = machine.preview(upload(files)).await;
    assert!(unchanged.skills[0].replaces);
    assert!(!unchanged.skills[0].changed);
    let mut replacement = skill("my-skill");
    replacement.retain(|f| f.path != "scripts/run");
    replacement.push(file("new.txt", b"new", false));
    let preview = machine.preview(upload(replacement.clone())).await;
    assert!(preview.skills[0].replaces && preview.skills[0].changed);
    machine.install(&preview).await;
    assert!(!folder.join("scripts/run").exists());
    assert_files(&folder, &replacement);
    let _: Value = machine
        .client
        .call(Call::RemoveSkill {
            name: "my-skill".into(),
        })
        .await
        .unwrap();
    assert!(!folder.exists());
    assert!(machine.list().await.installed.is_empty());
    assert!(
        machine.get(None, None).await["skills"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    machine.finish().await;
}

#[tokio::test]
async fn zip_preserves_files_and_rejects_unsafe_archives() {
    let machine = Machine::start().await;
    let mut files = skill("zip-skill");
    files.push(file(".DS_Store", b"ignored", false));
    files.push(file("__MACOSX/metadata", b"ignored", false));
    let preview = machine
        .preview(SkillFetch::Zip {
            label: "archive.zip".into(),
            base64: zip_files(&files),
        })
        .await;
    assert_eq!(preview.source, "archive.zip");
    machine.install(&preview).await;
    files.retain(|f| f.path != ".DS_Store" && !f.path.starts_with("__MACOSX/"));
    assert_files(&machine.state.join("skills/zip-skill"), &files);
    assert!(!machine.state.join("skills/zip-skill/.DS_Store").exists());
    for path in ["../escape", "/absolute", "a/../../escape", "a\\escape"] {
        let bad = vec![file(path, b"bad", false)];
        let result = machine
            .client
            .call::<Value>(Call::PreviewSkills {
                fetch: SkillFetch::Zip {
                    label: "bad.zip".into(),
                    base64: zip_files(&bad),
                },
            })
            .await;
        assert!(result.is_err(), "{path}");
    }
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    zip.add_symlink(
        "link",
        "/etc/passwd",
        zip::write::SimpleFileOptions::default(),
    )
    .unwrap();
    let encoded = BASE64.encode(zip.finish().unwrap().into_inner());
    let error = machine
        .client
        .call::<Value>(Call::PreviewSkills {
            fetch: SkillFetch::Zip {
                label: "link.zip".into(),
                base64: encoded,
            },
        })
        .await
        .unwrap_err();
    assert!(error.to_string().contains("symlink"));
    machine.finish().await;
}

#[tokio::test]
async fn rejects_invalid_metadata_paths_sizes_counts_and_duplicate_names() {
    let machine = Machine::start().await;
    for text in [
        "# no frontmatter",
        "---\nname: missing-description\n---\n",
        "---\ndescription: missing name\n---\n",
        "---\nname: [invalid]\ndescription: text\n---\n",
        "---\nname: ...\ndescription: text\n---\n",
    ] {
        let preview = machine
            .preview(upload(vec![file("SKILL.md", text.as_bytes(), false)]))
            .await;
        assert!(preview.skills.is_empty());
        assert_eq!(preview.refused.len(), 1);
    }
    let preview = machine
        .preview(upload(vec![file("README.md", b"not a skill", false)]))
        .await;
    assert!(preview.skills.is_empty());
    assert!(preview.refused[0].reason.contains("no SKILL.md"));
    let error = machine
        .client
        .call::<Value>(Call::PreviewSkills {
            fetch: upload(vec![file("../escape", b"bad", false)]),
        })
        .await
        .unwrap_err();
    assert!(error.to_string().contains("relative"));
    assert!(!machine.state.join("skills/escape").exists());
    let mut files = skill("too-large");
    files.push(file("large.bin", &vec![0; 20 * 1024 * 1024], false));
    let preview = machine.preview(upload(files)).await;
    assert!(preview.skills.is_empty());
    assert!(preview.refused[0].reason.contains("20 MB"));
    let mut files = skill("too-many");
    files.extend((0..2048).map(|n| file(&format!("data/{n}"), b"", false)));
    let preview = machine.preview(upload(files)).await;
    assert!(preview.skills.is_empty());
    assert!(preview.refused[0].reason.contains("2048 files"));
    let mut files = Vec::new();
    for prefix in ["a", "b"] {
        for mut item in skill("duplicate") {
            item.path = format!("{prefix}/{}", item.path);
            files.push(item);
        }
    }
    let preview = machine.preview(upload(files)).await;
    assert_eq!(preview.skills.len(), 1);
    assert_eq!(preview.refused.len(), 1);
    assert!(preview.refused[0].reason.contains("same name"));
    machine.finish().await;
}

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new(sandhill_daemon::agents::find("git").unwrap())
        .args(args)
        .current_dir(root)
        .env("GIT_AUTHOR_NAME", "Sandhill test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "Sandhill test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().into()
}

fn put_skill(root: &Path, name: &str) {
    fs::create_dir_all(root).unwrap();
    for file in skill(name) {
        let path = root.join(file.path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, BASE64.decode(file.base64).unwrap()).unwrap();
        fs::set_permissions(
            path,
            fs::Permissions::from_mode(if file.executable { 0o755 } else { 0o644 }),
        )
        .unwrap();
    }
}

#[tokio::test]
async fn local_git_pins_updates_licences_symlinks_and_copy() {
    let machine = Machine::start().await;
    let other = Machine::start().await;
    let dir = common::tempdir().unwrap();
    let repo = dir.path().join("repo");
    put_skill(&repo.join("one"), "git-one");
    fs::write(
        repo.join("LICENSE.txt"),
        "Apache License\nVersion 2.0, January 2004\n",
    )
    .unwrap();
    git(&repo, &["init", "--quiet"]);
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "--quiet", "-m", "first"]);
    let first = git(&repo, &["rev-parse", "HEAD"]);
    let url = format!("file://{}", repo.display());
    let preview = machine
        .preview(SkillFetch::Git {
            url: url.clone(),
            commit: Some(first.clone()),
        })
        .await;
    assert_eq!(preview.skills.len(), 1);
    assert_eq!(preview.skills[0].licence.as_deref(), Some("Apache-2.0"));
    assert!(preview.skills[0].files.iter().any(|p| p == "LICENSE"));
    let installed = machine.install(&preview).await;
    assert!(
        matches!(&installed[0].source, SkillSource::Git { path, commit, .. } if path == "one" && commit == &first)
    );
    assert_eq!(
        fs::read(machine.state.join("skills/git-one/LICENSE")).unwrap(),
        fs::read(repo.join("LICENSE.txt")).unwrap()
    );
    let unchanged = machine
        .preview(SkillFetch::Update {
            name: "git-one".into(),
        })
        .await;
    assert_eq!(unchanged.skills.len(), 1);
    assert!(!unchanged.skills[0].changed);

    put_skill(&repo.join("two"), "git-two");
    put_skill(&repo.join("unsafe"), "unsafe");
    symlink("/etc/passwd", repo.join("unsafe/link")).unwrap();
    fs::write(repo.join("one/references/notes.txt"), "changed reference\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "--quiet", "-m", "second"]);
    let second = git(&repo, &["rev-parse", "HEAD"]);
    let newest = machine
        .preview(SkillFetch::Git {
            url: url.clone(),
            commit: None,
        })
        .await;
    assert_eq!(newest.skills.len(), 2);
    assert_eq!(newest.refused.len(), 1);
    assert!(newest.refused[0].reason.contains("symlink"));
    let pinned = machine
        .preview(SkillFetch::Git {
            url,
            commit: Some(first.clone()),
        })
        .await;
    assert_eq!(pinned.skills.len(), 1);
    assert!(!pinned.skills[0].changed);
    let update = machine
        .preview(SkillFetch::Update {
            name: "git-one".into(),
        })
        .await;
    assert_eq!(update.skills.len(), 1);
    assert_eq!(update.skills[0].name, "git-one");
    assert!(update.skills[0].changed && update.skills[0].replaces);
    let installed = machine.install(&update).await;
    assert!(matches!(&installed[0].source, SkillSource::Git { commit, .. } if commit == &second));
    let exported: SkillExport = machine
        .client
        .call(Call::ExportSkill {
            name: "git-one".into(),
        })
        .await
        .unwrap();
    let preview = other
        .preview(SkillFetch::Files {
            label: "copy".into(),
            files: exported.files.clone(),
            source: Some(exported.source.clone()),
        })
        .await;
    let copied = other.install(&preview).await;
    assert_eq!(copied[0].source, exported.source);
    assert_files(&other.state.join("skills/git-one"), &exported.files);
    let reexported: SkillExport = other
        .client
        .call(Call::ExportSkill {
            name: "git-one".into(),
        })
        .await
        .unwrap();
    assert_eq!(reexported, exported);
    let update = other
        .preview(SkillFetch::Update {
            name: "git-one".into(),
        })
        .await;
    assert!(!update.skills[0].changed);
    machine.finish().await;
    other.finish().await;
}

#[tokio::test]
async fn tokens_are_consumed_expire_and_uploads_cannot_update() {
    let machine = Machine::start().await;
    let preview = machine.preview(upload(skill("token-test"))).await;
    let result = machine
        .client
        .call::<Value>(Call::InstallSkills {
            token: preview.token.clone(),
            names: vec!["unknown".into()],
        })
        .await;
    assert!(result.is_err());
    assert!(
        !machine
            .state
            .join("skills/.incoming")
            .join(&preview.token)
            .exists()
    );
    let error = machine
        .client
        .call::<Value>(Call::InstallSkills {
            token: preview.token,
            names: vec![],
        })
        .await
        .unwrap_err();
    assert!(error.to_string().contains("unknown or expired"));
    let preview = machine.preview(upload(skill("token-test"))).await;
    let manifest = machine
        .state
        .join("skills/.incoming")
        .join(&preview.token)
        .join("preview.json");
    let past = std::time::SystemTime::now() - Duration::from_secs(3601);
    fs::File::options()
        .write(true)
        .open(manifest)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(past))
        .unwrap();
    let error = machine
        .client
        .call::<Value>(Call::InstallSkills {
            token: preview.token,
            names: vec![],
        })
        .await
        .unwrap_err();
    assert!(error.to_string().contains("expired"));
    let preview = machine.preview(upload(skill("token-test"))).await;
    machine.install(&preview).await;
    let error = machine
        .client
        .call::<Value>(Call::PreviewSkills {
            fetch: SkillFetch::Update {
                name: "token-test".into(),
            },
        })
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("only skills from GitHub can be checked for updates")
    );
    machine.finish().await;
}
