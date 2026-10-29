//! The Git pane against a real git: a new repository, staging, committing,
//! diffs, branches, stashes, a remote, a merge that stops on a conflict, and
//! Write it with a stand-in agent. Skips (and says so) when git isn't
//! installed.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use sandhill_daemon::{Daemon, StateDir};
use sandhill_proto::client::Client;
use sandhill_proto::{GitAction, GitStatus};
use tokio::net::UnixListener;

struct TestDaemon {
    _daemon: Arc<Daemon>,
    socket: PathBuf,
}

impl TestDaemon {
    async fn start(state: &Path) -> TestDaemon {
        let daemon = Daemon::open(StateDir::new(state)).await.unwrap();
        let socket = state.join("test.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        tokio::spawn({
            let daemon = daemon.clone();
            async move {
                let _ = daemon.serve(listener).await;
            }
        });
        TestDaemon {
            _daemon: daemon,
            socket,
        }
    }

    async fn client(&self) -> Client {
        Client::connect_unix(&self.socket).await.unwrap()
    }
}

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

/// Files as (path, staged, unstaged).
fn files(status: &GitStatus) -> Vec<(&str, Option<&str>, Option<&str>)> {
    status
        .files
        .iter()
        .map(|f| (f.path.as_str(), f.staged.as_deref(), f.unstaged.as_deref()))
        .collect()
}

#[tokio::test]
async fn a_repository_is_made_changed_committed_and_pushed() {
    if std::process::Command::new("git")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("skipping: git is not installed");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir(&home).unwrap();
    let global = home.join(".gitconfig");
    std::fs::write(&global, "[init]\n\tdefaultBranch = main\n").unwrap();
    // SAFETY: this is the file's only test, so no other thread reads the
    // environment while it changes.
    unsafe {
        // The user's own git configuration stays out of it, and setting the
        // name and email writes here.
        std::env::set_var("GIT_CONFIG_GLOBAL", &global);
        std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
    }
    let state = dir.path().join("state");
    std::fs::create_dir(&state).unwrap();
    let daemon = TestDaemon::start(&state).await;
    let client = daemon.client().await;
    let project = dir.path().join("pbmc");
    std::fs::create_dir(&project).unwrap();
    let path = project.to_str().unwrap();

    // Not a repository until it is made one.
    let status = client.git_status(path).await.unwrap();
    assert_eq!(status.repo, "");
    client.git_run(path, GitAction::Init).await.unwrap();
    let status = client.git_status(path).await.unwrap();
    assert_eq!(
        Path::new(&status.repo).canonicalize().unwrap(),
        project.canonicalize().unwrap()
    );
    assert_eq!(status.branch.as_deref(), Some("main"));
    assert_eq!(status.head, None);
    assert_eq!(status.user_name, "");
    assert!(
        client
            .git_run(path, GitAction::Init)
            .await
            .unwrap_err()
            .to_string()
            .contains("already")
    );

    // A commit needs a name and an email.
    client
        .git_run(
            path,
            GitAction::SetUser {
                name: "Huan Yang".into(),
                email: "huan@example.org".into(),
            },
        )
        .await
        .unwrap();
    let status = client.git_status(path).await.unwrap();
    assert_eq!(
        (status.user_name.as_str(), status.user_email.as_str()),
        ("Huan Yang", "huan@example.org")
    );

    // New files: a script with its lines, and a folder as one entry.
    std::fs::write(project.join("qc.R"), "x <- 1\ny <- 2\n").unwrap();
    std::fs::create_dir(project.join("data")).unwrap();
    std::fs::write(project.join("data/counts.csv"), "a,b\n1,2\n").unwrap();
    let status = client.git_status(path).await.unwrap();
    assert_eq!(
        files(&status),
        [("data/", None, Some("?")), ("qc.R", None, Some("?"))]
    );
    let qc = &status.files[1];
    assert_eq!(qc.lines.unwrap().added, 2);
    assert_eq!(qc.size, Some(14));
    assert!(
        client
            .git_log(path, 0, None)
            .await
            .unwrap()
            .commits
            .is_empty()
    );
    let diff = client.git_diff(path, "qc.R", false, None).await.unwrap();
    assert_eq!(diff.text, "@@ -0,0 +1,2 @@\n+x <- 1\n+y <- 2\n");
    assert!(!diff.lines, "a new file is staged whole");

    // Staged, then committed.
    client
        .git_run(
            path,
            GitAction::Stage {
                files: vec!["qc.R".into()],
            },
        )
        .await
        .unwrap();
    let status = client.git_status(path).await.unwrap();
    assert_eq!(
        files(&status),
        [("data/", None, Some("?")), ("qc.R", Some("A"), None)]
    );
    assert_eq!(status.files[1].staged_lines.unwrap().added, 2);
    assert!(
        client
            .git_run(
                path,
                GitAction::Commit {
                    message: " ".into(),
                    amend: false
                }
            )
            .await
            .is_err()
    );
    client
        .git_run(
            path,
            GitAction::Commit {
                message: "Add QC script\n\nFilters cells by mitochondrial fraction.".into(),
                amend: false,
            },
        )
        .await
        .unwrap();
    let status = client.git_status(path).await.unwrap();
    assert!(status.head.is_some());
    assert_eq!(files(&status), [("data/", None, Some("?"))]);
    let log = client.git_log(path, 0, None).await.unwrap();
    assert_eq!(log.commits.len(), 1);
    let first = &log.commits[0];
    assert_eq!(first.subject, "Add QC script");
    assert_eq!(first.author, "Huan Yang");
    assert_eq!(
        (first.refs[0].kind.as_str(), first.refs[0].name.as_str()),
        ("head", "main")
    );
    let shown = client.git_show(path, &first.sha).await.unwrap();
    assert_eq!(
        shown.message,
        "Add QC script\n\nFilters cells by mitochondrial fraction."
    );
    assert_eq!(shown.files.len(), 1);
    assert_eq!(
        (shown.files[0].path.as_str(), shown.files[0].status.as_str()),
        ("qc.R", "A")
    );
    let diff = client
        .git_diff(path, "qc.R", false, Some(&first.sha))
        .await
        .unwrap();
    assert!(diff.text.starts_with("@@ -0,0 +1,2 @@"), "{diff:?}");
    assert!(!diff.lines);
    assert!(client.git_show(path, "--output=x").await.is_err());

    // A change: its diff before and after staging, then unstaged and
    // thrown away.
    std::fs::write(project.join("qc.R"), "x <- 1\ny <- 3\n").unwrap();
    let status = client.git_status(path).await.unwrap();
    let lines = status.files[1].lines.unwrap();
    assert_eq!((lines.added, lines.removed), (1, 1));
    let diff = client.git_diff(path, "qc.R", false, None).await.unwrap();
    assert!(diff.text.contains("-y <- 2\n+y <- 3\n"), "{diff:?}");
    client
        .git_run(path, GitAction::Stage { files: vec![] })
        .await
        .unwrap();
    let status = client.git_status(path).await.unwrap();
    assert_eq!(
        files(&status),
        [
            ("data/counts.csv", Some("A"), None),
            ("qc.R", Some("M"), None)
        ]
    );
    assert!(
        client
            .git_diff(path, "qc.R", true, None)
            .await
            .unwrap()
            .text
            .contains("+y <- 3")
    );
    client
        .git_run(path, GitAction::Unstage { files: vec![] })
        .await
        .unwrap();
    client
        .git_run(
            path,
            GitAction::Discard {
                files: vec!["qc.R".into(), "data/".into()],
            },
        )
        .await
        .unwrap();
    let status = client.git_status(path).await.unwrap();
    assert!(status.files.is_empty(), "{:?}", status.files);
    assert!(!project.join("data").exists());
    assert_eq!(
        std::fs::read_to_string(project.join("qc.R")).unwrap(),
        "x <- 1\ny <- 2\n"
    );

    // Some lines of a change staged, unstaged and thrown away, by their
    // places in the diff shown; a diff the file no longer matches is
    // refused.
    std::fs::write(project.join("qc.R"), "x <- 10\ny <- 20\nz <- 3\n").unwrap();
    let place = |diff: &str, line: &str| diff.split('\n').position(|l| l == line).unwrap();
    let lines = |kind: &str, diff: &sandhill_proto::GitDiff, picked: &[&str]| {
        let (file, diff, lines) = (
            "qc.R".to_string(),
            diff.text.clone(),
            picked.iter().map(|line| place(&diff.text, line)).collect(),
        );
        match kind {
            "stage" => GitAction::StageLines { file, diff, lines },
            "unstage" => GitAction::UnstageLines { file, diff, lines },
            _ => GitAction::DiscardLines { file, diff, lines },
        }
    };
    let index = || git(&project, &["show", ":qc.R"]);
    let worktree = || std::fs::read_to_string(project.join("qc.R")).unwrap();
    let diff = client.git_diff(path, "qc.R", false, None).await.unwrap();
    assert!(diff.lines, "{diff:?}");
    client
        .git_run(path, lines("stage", &diff, &["-x <- 1", "+x <- 10"]))
        .await
        .unwrap();
    assert_eq!(index(), "x <- 10\ny <- 2\n");
    let diff = client.git_diff(path, "qc.R", false, None).await.unwrap();
    client
        .git_run(path, lines("stage", &diff, &["+z <- 3"]))
        .await
        .unwrap();
    assert_eq!(index(), "x <- 10\ny <- 2\nz <- 3\n");
    let staged = client.git_diff(path, "qc.R", true, None).await.unwrap();
    assert!(staged.lines, "{staged:?}");
    client
        .git_run(path, lines("unstage", &staged, &["-x <- 1", "+x <- 10"]))
        .await
        .unwrap();
    assert_eq!(index(), "x <- 1\ny <- 2\nz <- 3\n");
    let diff = client.git_diff(path, "qc.R", false, None).await.unwrap();
    client
        .git_run(path, lines("discard", &diff, &["-y <- 2", "+y <- 20"]))
        .await
        .unwrap();
    assert_eq!(worktree(), "x <- 10\ny <- 2\nz <- 3\n");
    assert_eq!(index(), "x <- 1\ny <- 2\nz <- 3\n");
    let err = client
        .git_run(path, lines("discard", &diff, &["-y <- 2", "+y <- 20"]))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("changed since"), "{err}");
    client
        .git_run(path, GitAction::Unstage { files: vec![] })
        .await
        .unwrap();
    client
        .git_run(
            path,
            GitAction::Discard {
                files: vec!["qc.R".into()],
            },
        )
        .await
        .unwrap();
    assert!(client.git_status(path).await.unwrap().files.is_empty());

    // A branch with a commit, undone and made again.
    client
        .git_run(
            path,
            GitAction::CreateBranch {
                name: "exp/harmony".into(),
            },
        )
        .await
        .unwrap();
    std::fs::write(project.join("harmony.R"), "library(harmony)\n").unwrap();
    client
        .git_run(path, GitAction::Stage { files: vec![] })
        .await
        .unwrap();
    let commit = |message: &str| GitAction::Commit {
        message: message.into(),
        amend: false,
    };
    client.git_run(path, commit("Try harmony")).await.unwrap();
    client.git_run(path, GitAction::UndoCommit).await.unwrap();
    let status = client.git_status(path).await.unwrap();
    assert_eq!(files(&status), [("harmony.R", Some("A"), None)]);
    client
        .git_run(path, commit("Try harmony integration"))
        .await
        .unwrap();
    client
        .git_run(
            path,
            GitAction::Switch {
                branch: "main".into(),
            },
        )
        .await
        .unwrap();
    let status = client.git_status(path).await.unwrap();
    assert_eq!(status.branch.as_deref(), Some("main"));
    assert_eq!(status.branches, ["exp/harmony", "main"]);
    let log = client.git_log(path, 0, None).await.unwrap();
    assert_eq!(log.commits.len(), 2);
    assert_eq!(log.commits[0].subject, "Try harmony integration");
    assert_eq!(log.commits[0].refs[0].name, "exp/harmony");
    assert_eq!(log.commits[0].parents, [log.commits[1].sha.clone()]);
    let page = client.git_log(path, 1, Some(1)).await.unwrap();
    assert_eq!(page.commits[0].subject, "Add QC script");
    assert!(!page.more);
    assert!(client.git_log(path, 0, Some(1)).await.unwrap().more);

    // Stashed and brought back.
    std::fs::write(project.join("qc.R"), "x <- 10\ny <- 2\n").unwrap();
    client
        .git_run(path, GitAction::Stash { message: None })
        .await
        .unwrap();
    let status = client.git_status(path).await.unwrap();
    assert!(status.files.is_empty());
    assert_eq!(status.stashes, 1);
    client.git_run(path, GitAction::StashPop).await.unwrap();
    let status = client.git_status(path).await.unwrap();
    assert_eq!(files(&status), [("qc.R", None, Some("M"))]);
    assert_eq!(status.stashes, 0);
    client
        .git_run(
            path,
            GitAction::Discard {
                files: vec!["qc.R".into()],
            },
        )
        .await
        .unwrap();

    // Pushing needs a remote; the first push sets the upstream.
    let err = client.git_run(path, GitAction::Push).await.unwrap_err();
    assert!(err.to_string().contains("no remote"), "{err}");
    let remote = dir.path().join("remote.git");
    git(
        dir.path(),
        &["init", "-q", "--bare", remote.to_str().unwrap()],
    );
    git(
        &project,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    client.git_run(path, GitAction::Push).await.unwrap();
    let status = client.git_status(path).await.unwrap();
    assert_eq!(status.upstream.as_deref(), Some("origin/main"));
    assert_eq!(status.remote_branches, ["origin/main"]);
    assert_eq!((status.ahead, status.behind), (0, 0));
    std::fs::write(project.join("README.md"), "# PBMC\n").unwrap();
    client
        .git_run(path, GitAction::Stage { files: vec![] })
        .await
        .unwrap();
    client.git_run(path, commit("Add a README")).await.unwrap();
    assert_eq!(client.git_status(path).await.unwrap().ahead, 1);
    client.git_run(path, GitAction::Push).await.unwrap();
    client.git_run(path, GitAction::Fetch).await.unwrap();
    client
        .git_run(path, GitAction::Pull { rebase: false })
        .await
        .unwrap();
    let status = client.git_status(path).await.unwrap();
    assert_eq!((status.ahead, status.behind), (0, 0));
    let log = client.git_log(path, 0, None).await.unwrap();
    let readme = log
        .commits
        .iter()
        .find(|c| c.subject == "Add a README")
        .unwrap();
    let kinds: Vec<_> = readme
        .refs
        .iter()
        .map(|r| (r.kind.as_str(), r.name.as_str()))
        .collect();
    assert_eq!(kinds, [("head", "main"), ("remote", "origin/main")]);

    // A merge that stops on a conflict, and stopping it.
    git(&project, &["checkout", "-q", "exp/harmony"]);
    std::fs::write(project.join("qc.R"), "x <- 1\ny <- 20\n").unwrap();
    git(&project, &["commit", "-q", "-am", "Change y on the branch"]);
    git(&project, &["checkout", "-q", "main"]);
    std::fs::write(project.join("qc.R"), "x <- 1\ny <- 30\n").unwrap();
    git(&project, &["commit", "-q", "-am", "Change y on main"]);
    let merge = |branch: &str| GitAction::Merge {
        branch: branch.into(),
    };
    for wrong in ["main", "nope", "--abort"] {
        assert!(client.git_run(path, merge(wrong)).await.is_err(), "{wrong}");
    }
    let done = client.git_run(path, merge("exp/harmony")).await.unwrap();
    assert!(done.output.contains("CONFLICT"), "{done:?}");
    let status = client.git_status(path).await.unwrap();
    assert_eq!(status.state.as_deref(), Some("merging"));
    assert!(
        status
            .merge_message
            .as_deref()
            .unwrap()
            .starts_with("Merge branch 'exp/harmony'"),
        "{:?}",
        status.merge_message
    );
    let conflicted: Vec<_> = status.files.iter().filter(|f| f.conflict).collect();
    assert_eq!(conflicted.len(), 1);
    assert_eq!(conflicted[0].path, "qc.R");
    client.git_run(path, GitAction::Abort).await.unwrap();
    let status = client.git_status(path).await.unwrap();
    assert_eq!(status.state, None);
    assert_eq!(status.merge_message, None);
    assert!(status.files.is_empty());

    // Merged again and settled by taking the branch's side, then committed
    // with git's message: a commit with both parents.
    client.git_run(path, merge("exp/harmony")).await.unwrap();
    client
        .git_run(
            path,
            GitAction::Resolve {
                file: "qc.R".into(),
                side: sandhill_proto::GitSide::Theirs,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(project.join("qc.R")).unwrap(),
        "x <- 1\ny <- 20\n"
    );
    let status = client.git_status(path).await.unwrap();
    assert_eq!(
        files(&status),
        [("harmony.R", Some("A"), None), ("qc.R", Some("M"), None)]
    );
    let message = status.merge_message.unwrap();
    client.git_run(path, commit(&message)).await.unwrap();
    let status = client.git_status(path).await.unwrap();
    assert_eq!(status.state, None);
    let log = client.git_log(path, 0, None).await.unwrap();
    assert_eq!(log.commits[0].subject, "Merge branch 'exp/harmony'");
    assert_eq!(log.commits[0].parents.len(), 2);
    // A branch with nothing new merges as nothing.
    let done = client.git_run(path, merge("exp/harmony")).await.unwrap();
    assert!(done.output.contains("up to date"), "{done:?}");

    // Write it, with a stand-in for Claude Code that answers from a script.
    let bin = dir.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let fake = bin.join("claude");
    std::fs::write(
        &fake,
        "#!/bin/sh\n\
         if [ \"$1\" = --help ]; then echo '--model --tools --no-session-persistence'; exit 0; fi\n\
         echo \"$@\" > \"$(dirname \"$0\")/args.txt\"\n\
         cat > \"$(dirname \"$0\")/input.txt\"\n\
         printf '```\\nPin Seurat in the README\\n\\nSo the analysis reruns the same.\\n```\\n'\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path_var = std::env::var_os("PATH").unwrap_or_default();
    let joined =
        std::env::join_paths(std::iter::once(bin.clone()).chain(std::env::split_paths(&path_var)))
            .unwrap();
    // SAFETY: as above, nothing else reads the environment meanwhile.
    unsafe { std::env::set_var("PATH", joined) };
    let err = client.git_write_message(path, None).await.unwrap_err();
    assert!(err.to_string().contains("stage the changes"), "{err}");
    std::fs::write(project.join("README.md"), "# PBMC\n\nSeurat 5.1\n").unwrap();
    client
        .git_run(path, GitAction::Stage { files: vec![] })
        .await
        .unwrap();
    let message = client.git_write_message(path, None).await.unwrap();
    assert_eq!(message.agent, "claude");
    assert_eq!(message.summary, "Pin Seurat in the README");
    assert_eq!(message.body, "So the analysis reruns the same.");
    let input = std::fs::read_to_string(bin.join("input.txt")).unwrap();
    assert!(
        input.contains("Recent commit subjects:\nMerge branch 'exp/harmony'\n"),
        "{input}"
    );
    assert!(input.contains("+Seurat 5.1"), "{input}");
    let args = std::fs::read_to_string(bin.join("args.txt")).unwrap();
    assert!(args.starts_with("-p Write a git commit message"), "{args}");
    assert!(args.contains("--model haiku"), "{args}");
    assert!(!args.contains("--strict-mcp-config"), "{args}");
}
