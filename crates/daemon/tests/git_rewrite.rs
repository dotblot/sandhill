//! Squashing, reordering and rewording the branch's own commits, against a
//! real git: only the commits not pushed are offered, and a rewrite makes
//! them again in the order asked, keeping changes not committed. Skips (and
//! says so) when git isn't installed.

use std::path::Path;

use sandhill_daemon::{Daemon, StateDir};
use sandhill_proto::client::Client;
use sandhill_proto::{GitAction, GitRewrite};
use tokio::net::UnixListener;

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
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn commit(dir: &Path, file: &str, text: &str, message: &str) -> String {
    std::fs::write(dir.join(file), text).unwrap();
    git(dir, &["add", file]);
    git(dir, &["commit", "-q", "-m", message]);
    git(dir, &["rev-parse", "HEAD"])
}

#[tokio::test]
async fn own_commits_are_squashed_reordered_and_reworded() {
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
    let daemon = Daemon::open(StateDir::new(&state)).await.unwrap();
    let socket = state.join("test.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    tokio::spawn(async move {
        let _ = daemon.serve(listener).await;
    });
    let client = Client::connect_unix(&socket).await.unwrap();

    // A project in a folder with a space, its first commit pushed.
    let project = dir.path().join("lab work/pbmc");
    std::fs::create_dir_all(&project).unwrap();
    let path = project.to_str().unwrap();
    let remote = dir.path().join("remote.git");
    git(
        dir.path(),
        &["init", "-q", "--bare", remote.to_str().unwrap()],
    );
    git(&project, &["init", "-q"]);
    let pushed = commit(&project, "qc.R", "x <- 1\n", "Add QC");
    git(
        &project,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(&project, &["push", "-q", "-u", "origin", "main"]);
    let umap = commit(&project, "umap.R", "umpa()\n", "Add UMAP");
    let typo = commit(&project, "umap.R", "umap()\n", "Fix typo");
    let pca = commit(&project, "pca.R", "pca()\n", "Add PCA");

    let own = client.git_own_commits(path).await.unwrap();
    let shas: Vec<&str> = own.commits.iter().map(|c| c.sha.as_str()).collect();
    assert_eq!(shas, [umap.as_str(), typo.as_str(), pca.as_str()]);
    assert_eq!(own.base.as_deref(), Some(pushed.as_str()));
    assert_eq!(own.commits[1].message, "Fix typo");
    assert!(own.note.unwrap().contains("on the remote already"));

    let step = |sha: &str, squash: bool, message: Option<&str>| GitRewrite {
        sha: sha.into(),
        squash,
        message: message.map(str::to_string),
    };
    let rewrite = |commits: Vec<GitRewrite>| GitAction::Rewrite {
        base: own.base.clone(),
        commits,
    };
    // Every commit, once, and the first with nothing to squash into.
    for wrong in [
        vec![step(&umap, false, None), step(&pca, false, None)],
        vec![
            step(&umap, true, None),
            step(&typo, false, None),
            step(&pca, false, None),
        ],
        vec![
            step(&umap, false, None),
            step(&umap, false, None),
            step(&pca, false, None),
        ],
    ] {
        assert!(client.git_run(path, rewrite(wrong)).await.is_err());
    }
    assert_eq!(git(&project, &["rev-parse", "HEAD"]), pca);

    // PCA first, then UMAP with its typo fix folded in and a new message;
    // a change not committed stays.
    std::fs::write(project.join("qc.R"), "x <- 2\n").unwrap();
    let message = "Add UMAP\n\nWith the typo fixed.";
    client
        .git_run(
            path,
            rewrite(vec![
                step(&pca, false, None),
                step(&umap, false, Some(message)),
                step(&typo, true, None),
            ]),
        )
        .await
        .unwrap();
    assert_eq!(
        git(&project, &["log", "--format=%s", "main"]),
        "Add UMAP\nAdd PCA\nAdd QC"
    );
    assert_eq!(git(&project, &["log", "-1", "--format=%B"]), message);
    assert_eq!(git(&project, &["rev-parse", "HEAD~2"]), pushed);
    assert_eq!(
        std::fs::read_to_string(project.join("umap.R")).unwrap(),
        "umap()\n"
    );
    assert_eq!(
        std::fs::read_to_string(project.join("qc.R")).unwrap(),
        "x <- 2\n"
    );
    let status = client.git_status(path).await.unwrap();
    assert_eq!(status.state, None);
    assert_eq!(status.ahead, 2);
}
