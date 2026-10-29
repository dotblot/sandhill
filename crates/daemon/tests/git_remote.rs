//! The Git pane's rebasing, tags and signing in, against a real git: a
//! branch rebased onto main, cleanly and through a conflict; a tag made,
//! pushed and deleted; and a remote that wants a password, which git asks
//! the window for through `sandhilld` as its askpass program. Skips (and
//! says so) when git isn't installed.

use std::path::Path;
use std::sync::{Arc, Mutex};

use base64::Engine;
use sandhill_daemon::{Daemon, DaemonOptions, StateDir};
use sandhill_proto::client::Client;
use sandhill_proto::{Call, GitAction, GitSide};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UnixListener};

/// Runs git directly, as the user would in a terminal.
fn git(dir: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn commit(dir: &Path, file: &str, text: &str, message: &str) {
    std::fs::write(dir.join(file), text).unwrap();
    git(dir, &["add", file]);
    git(dir, &["commit", "-q", "-m", message]);
}

/// A web server that wants a password and takes none, keeping the
/// `Authorization` header of each request.
async fn locked_server() -> (u16, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    tokio::spawn({
        let seen = seen.clone();
        async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut request = Vec::new();
                let mut buf = [0u8; 4096];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => request.extend_from_slice(&buf[..n]),
                    }
                }
                let request = String::from_utf8_lossy(&request).into_owned();
                let auth = request
                    .lines()
                    .find_map(|l| l.strip_prefix("Authorization: "))
                    .unwrap_or("")
                    .to_string();
                seen.lock().unwrap().push(auth);
                let _ = stream
                    .write_all(
                        b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"lab\"\r\n\
                          Content-Length: 0\r\nConnection: close\r\n\r\n",
                    )
                    .await;
            }
        }
    });
    (port, seen)
}

#[tokio::test]
async fn branches_are_rebased_tags_pushed_and_passwords_asked_for() {
    if std::process::Command::new("git")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("skipping: git is not installed");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let global = dir.path().join(".gitconfig");
    std::fs::write(
        &global,
        "[init]\n\tdefaultBranch = main\n[user]\n\tname = Huan Yang\n\temail = huan@example.org\n",
    )
    .unwrap();
    // SAFETY: this is the file's only test, so no other thread reads the
    // environment while it changes.
    unsafe {
        std::env::set_var("GIT_CONFIG_GLOBAL", &global);
        std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
    }
    let state = dir.path().join("state");
    let socket = state.join("test.sock");
    let options = DaemonOptions {
        executable: env!("CARGO_BIN_EXE_sandhilld").into(),
        socket: socket.clone(),
        started_by: String::new(),
    };
    let daemon = Daemon::open_with(StateDir::new(&state), options)
        .await
        .unwrap();
    let listener = UnixListener::bind(&socket).unwrap();
    tokio::spawn(async move {
        let _ = daemon.serve(listener).await;
    });
    let mut client = Client::connect_unix(&socket).await.unwrap();
    let mut requests = client.take_requests().unwrap();
    let client = Arc::new(client);

    let project = dir.path().join("pbmc");
    std::fs::create_dir(&project).unwrap();
    let path = project.to_str().unwrap();
    git(&project, &["init", "-q"]);
    commit(&project, "qc.R", "x <- 1\ny <- 2\n", "Add QC");

    // A branch rebased onto main: its commit goes on top of main's.
    git(&project, &["checkout", "-q", "-b", "exp"]);
    commit(&project, "umap.R", "umap()\n", "Add UMAP");
    git(&project, &["checkout", "-q", "main"]);
    commit(&project, "pca.R", "pca()\n", "Add PCA");
    git(&project, &["checkout", "-q", "exp"]);
    let rebase = |branch: &str| GitAction::Rebase {
        branch: branch.into(),
    };
    for wrong in ["exp", "nope", "--onto"] {
        assert!(
            client.git_run(path, rebase(wrong)).await.is_err(),
            "{wrong}"
        );
    }
    client.git_run(path, rebase("main")).await.unwrap();
    let parent = git(&project, &["rev-parse", "HEAD~1"]);
    assert_eq!(parent, git(&project, &["rev-parse", "main"]));

    // Through a conflict: the branch's own version is kept, and it goes on.
    commit(&project, "qc.R", "x <- 1\ny <- 20\n", "Change y on exp");
    git(&project, &["checkout", "-q", "main"]);
    commit(&project, "qc.R", "x <- 1\ny <- 30\n", "Change y on main");
    git(&project, &["checkout", "-q", "exp"]);
    client.git_run(path, rebase("main")).await.unwrap();
    let status = client.git_status(path).await.unwrap();
    assert_eq!(status.state.as_deref(), Some("rebasing"));
    assert!(status.files.iter().any(|f| f.path == "qc.R" && f.conflict));
    let mine = GitAction::Resolve {
        file: "qc.R".into(),
        side: GitSide::Ours,
    };
    client.git_run(path, mine).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(project.join("qc.R")).unwrap(),
        "x <- 1\ny <- 20\n"
    );
    client.git_run(path, GitAction::Continue).await.unwrap();
    let status = client.git_status(path).await.unwrap();
    assert_eq!(
        (status.state, status.branch.as_deref()),
        (None, Some("exp"))
    );
    assert_eq!(
        git(&project, &["log", "--format=%s", "-3"]),
        "Change y on exp\nAdd UMAP\nChange y on main\n"
    );

    // A tag, pushed to the remote, then deleted here.
    let remote = dir.path().join("remote.git");
    git(
        dir.path(),
        &["init", "-q", "--bare", remote.to_str().unwrap()],
    );
    git(
        &project,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    for wrong in ["", "-d", "a b", "v1..0"] {
        let tag = GitAction::CreateTag {
            name: wrong.into(),
            sha: None,
        };
        assert!(client.git_run(path, tag).await.is_err(), "{wrong:?}");
    }
    let tag = GitAction::CreateTag {
        name: "v1.0".into(),
        sha: None,
    };
    client.git_run(path, tag).await.unwrap();
    let status = client.git_status(path).await.unwrap();
    assert_eq!(status.tags, ["v1.0"]);
    let push = GitAction::PushTag {
        name: "v1.0".into(),
    };
    client.git_run(path, push).await.unwrap();
    assert!(git(&remote, &["tag"]).contains("v1.0"));
    let delete = GitAction::DeleteTag {
        name: "v1.0".into(),
    };
    client.git_run(path, delete).await.unwrap();
    assert!(client.git_status(path).await.unwrap().tags.is_empty());

    // A remote that wants a password: git asks the window, and sends what
    // was typed.
    let (port, seen) = locked_server().await;
    let url = format!("http://127.0.0.1:{port}/lab/pbmc.git");
    git(&project, &["remote", "add", "lab", &url]);
    let asked = Arc::new(Mutex::new(Vec::new()));
    tokio::spawn({
        let client = client.clone();
        let asked = asked.clone();
        async move {
            while let Some(request) = requests.recv().await {
                let prompt = request.params["prompt"].as_str().unwrap_or("").to_string();
                let typed = if prompt.starts_with("Username") {
                    "huan"
                } else {
                    "s3cret"
                };
                asked.lock().unwrap().push((
                    request.method.clone(),
                    prompt,
                    request.params["secret"].clone(),
                ));
                let _: Value = client
                    .call(Call::Answer {
                        id: request.id,
                        result: Some(json!(typed)),
                        error: None,
                    })
                    .await
                    .unwrap();
            }
        }
    });
    let err = client.git_run(path, GitAction::Fetch).await.unwrap_err();
    assert!(err.to_string().contains("Authentication failed"), "{err}");
    let host = format!("127.0.0.1:{port}");
    assert_eq!(
        *asked.lock().unwrap(),
        [
            (
                "git_credential".to_string(),
                format!("Username for 'http://{host}':"),
                json!(false)
            ),
            (
                "git_credential".to_string(),
                format!("Password for 'http://huan@{host}':"),
                json!(true)
            ),
        ]
    );
    let basic = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode("huan:s3cret")
    );
    assert!(seen.lock().unwrap().contains(&basic), "{seen:?}");
}
