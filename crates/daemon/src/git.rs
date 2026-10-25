//! Git for the Git pane: a project's status, history and changes. The daemon
//! runs the `git` command on its own host, so the user's configuration,
//! credentials and hooks apply, and it works the same locally and over ssh.
//! Git never waits for a password: it runs without a terminal and with
//! prompts off, so a push that needs one fails with git's own message.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use sandhill_proto::{
    GitAction, GitChange, GitCommit, GitCommitInfo, GitDiff, GitDone, GitFile, GitLines, GitLog,
    GitMessage, GitRef, GitSide, GitStatus,
};
use tokio::io::AsyncWriteExt;

use crate::files;
use crate::patch::{self, Way};

/// Files a status lists, at most.
const MAX_FILES: usize = 2000;
/// Commits in a page of the log, unless asked, and at most.
const LOG_PAGE: u32 = 200;
const MAX_LOG: u32 = 2000;
/// A diff is cut at this many bytes.
const MAX_DIFF: usize = 2 * 1024 * 1024;
/// A new file is read to count its lines only up to this size.
const COUNT_MAX: u64 = 4 * 1024 * 1024;
/// How long git may take to read, and to change things (hooks run then) or
/// talk to a remote.
const READ_TIMEOUT: Duration = Duration::from_secs(60);
const WRITE_TIMEOUT: Duration = Duration::from_secs(300);
/// What an agent sees of the staged changes when writing a message.
const PROMPT_FILE: usize = 4000;
const PROMPT_TOTAL: usize = 40_000;
const AGENT_TIMEOUT: Duration = Duration::from_secs(180);

/// What git printed, and whether it succeeded.
struct Output {
    stdout: String,
    stderr: String,
    code: Option<i32>,
}

impl Output {
    fn ok(&self) -> bool {
        self.code == Some(0)
    }

    /// Git's message on failure: its last lines of stderr, or stdout's.
    fn error(&self) -> String {
        let text = if self.stderr.trim().is_empty() {
            &self.stdout
        } else {
            &self.stderr
        };
        let lines: Vec<&str> = text
            .lines()
            .map(str::trim_end)
            .filter(|l| !l.trim().is_empty())
            .collect();
        let tail = &lines[lines.len().saturating_sub(12)..];
        let text = tail.join("\n");
        let text = text.strip_prefix("fatal: ").unwrap_or(&text);
        if text.is_empty() {
            format!("git stopped with code {}", self.code.unwrap_or(-1))
        } else {
            text.to_string()
        }
    }
}

/// The `git` to run.
fn program() -> Result<PathBuf> {
    let git = crate::agents::find("git")
        .context("git is not installed on this machine; install it to use the Git pane")?;
    // macOS's /usr/bin/git only offers to install Apple's command line tools
    // until they are.
    if cfg!(target_os = "macos") && git == Path::new("/usr/bin/git") {
        let tools = std::process::Command::new("/usr/bin/xcode-select")
            .arg("-p")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if !tools {
            bail!(
                "git needs Apple's command line tools; install them with \
                 `xcode-select --install` in a terminal"
            );
        }
    }
    Ok(git)
}

/// Runs git in `dir`. Its exit code is in the output; only failing to run it
/// at all is an error.
async fn run(dir: &Path, args: &[&str], input: Option<&str>, timeout: Duration) -> Result<Output> {
    let mut command = tokio::process::Command::new(program()?);
    command
        .args(["-c", "core.quotepath=off", "-c", "color.ui=false"])
        .args(args)
        .current_dir(dir)
        // Never ask for a password or passphrase: there is no one to answer.
        .env("GIT_TERMINAL_PROMPT", "0")
        // Reading the status mustn't take the index lock from the user's own git.
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    // SAFETY: setsid is async-signal-safe, as code between fork and exec
    // must be. Without a controlling terminal, ssh can't ask for a passphrase.
    unsafe {
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let mut child = command.spawn().context("cannot run git")?;
    if let Some(input) = input
        && let Some(mut stdin) = child.stdin.take()
    {
        stdin.write_all(input.as_bytes()).await?;
    }
    let output = tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .map_err(|_| anyhow!("git {} took too long", args.first().unwrap_or(&"")))??;
    Ok(Output {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        code: output.status.code(),
    })
}

/// Runs git, failing with its message when it fails.
async fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let output = run(dir, args, None, READ_TIMEOUT).await?;
    if !output.ok() {
        bail!("{}", output.error());
    }
    Ok(output.stdout)
}

/// Runs git to change something, returning what it said.
async fn change(dir: &Path, args: &[&str], input: Option<&str>) -> Result<String> {
    let output = run(dir, args, input, WRITE_TIMEOUT).await?;
    if !output.ok() {
        bail!("{}", output.error());
    }
    let mut said = output.stdout.trim().to_string();
    let err = output.stderr.trim();
    if !err.is_empty() {
        if !said.is_empty() {
            said.push('\n');
        }
        said.push_str(err);
    }
    Ok(said)
}

struct Repo {
    root: PathBuf,
    git_dir: PathBuf,
}

/// The repository holding `path`, if it is in one.
async fn repo(path: &str) -> Result<Option<Repo>> {
    let dir = files::resolve(path)?;
    if !dir.is_dir() {
        bail!("{} is not a folder", dir.display());
    }
    let output = run(
        &dir,
        &["rev-parse", "--show-toplevel", "--absolute-git-dir"],
        None,
        READ_TIMEOUT,
    )
    .await?;
    if !output.ok() {
        if output.stderr.contains("not a git repository") {
            return Ok(None);
        }
        bail!("{}", output.error());
    }
    let mut lines = output.stdout.lines();
    match (lines.next(), lines.next()) {
        (Some(root), Some(git_dir)) if !root.is_empty() => Ok(Some(Repo {
            root: PathBuf::from(root),
            git_dir: PathBuf::from(git_dir),
        })),
        // A bare repository, or the inside of .git: nothing to show.
        _ => Ok(None),
    }
}

async fn need_repo(path: &str) -> Result<Repo> {
    repo(path)
        .await?
        .with_context(|| format!("{path} is not in a Git repository"))
}

pub async fn status(path: &str) -> Result<GitStatus> {
    let Some(repo) = repo(path).await? else {
        return Ok(GitStatus::default());
    };
    let dir = &repo.root;
    let (status, staged, unstaged, refs, remotes, user, stashes) = tokio::join!(
        git(
            dir,
            &[
                "status",
                "--porcelain=v2",
                "--branch",
                "-z",
                "--untracked-files=normal"
            ]
        ),
        run(
            dir,
            &["diff", "--cached", "--numstat", "-z"],
            None,
            READ_TIMEOUT
        ),
        run(dir, &["diff", "--numstat", "-z"], None, READ_TIMEOUT),
        git(
            dir,
            &[
                "for-each-ref",
                "--format=%(objectname) %(refname)",
                "refs/heads",
                "refs/remotes",
                "refs/tags",
            ]
        ),
        git(dir, &["remote"]),
        run(
            dir,
            &["config", "--get-regexp", r"^user\.(name|email)$"],
            None,
            READ_TIMEOUT
        ),
        run(dir, &["stash", "list", "--format=%h"], None, READ_TIMEOUT),
    );
    let mut status = parse_status(&status?);
    status.repo = repo.root.to_string_lossy().into_owned();

    let staged = staged?;
    let unstaged = unstaged?;
    let staged = if staged.ok() {
        parse_numstat(&staged.stdout)
    } else {
        HashMap::new()
    };
    let unstaged = if unstaged.ok() {
        parse_numstat(&unstaged.stdout)
    } else {
        HashMap::new()
    };
    for file in &mut status.files {
        if file.staged.is_some() {
            file.staged_lines = staged.get(&file.path).copied();
        }
        match file.unstaged.as_deref() {
            Some("?") => {
                let full = repo.root.join(&file.path);
                if !file.path.ends_with('/') {
                    file.size = std::fs::metadata(&full).ok().map(|m| m.len());
                    file.lines = count_lines(&full);
                }
            }
            Some(_) => file.lines = unstaged.get(&file.path).copied(),
            None => {}
        }
        if file.staged.as_deref() == Some("A") {
            file.size = std::fs::metadata(repo.root.join(&file.path))
                .ok()
                .map(|m| m.len());
        }
    }

    let refs = refs?;
    let mut hasher = DefaultHasher::new();
    refs.hash(&mut hasher);
    status.head.hash(&mut hasher);
    status.refs = format!("{:016x}", hasher.finish());
    for line in refs.lines() {
        let Some((_, name)) = line.split_once(' ') else {
            continue;
        };
        if let Some(branch) = name.strip_prefix("refs/heads/") {
            status.branches.push(branch.to_string());
        } else if let Some(remote) = name.strip_prefix("refs/remotes/")
            && !remote.ends_with("/HEAD")
        {
            status.remote_branches.push(remote.to_string());
        }
    }
    status.remotes = remotes?.lines().map(str::to_string).collect();
    let user = user?;
    for line in user.stdout.lines() {
        match line.split_once(' ') {
            Some(("user.name", name)) => status.user_name = name.to_string(),
            Some(("user.email", email)) => status.user_email = email.to_string(),
            _ => {}
        }
    }
    let stashes = stashes?;
    if stashes.ok() {
        status.stashes = stashes.stdout.lines().count() as u32;
    }
    status.state = state(&repo.git_dir).map(str::to_string);
    if status.state.as_deref() == Some("merging") {
        status.merge_message = std::fs::read_to_string(repo.git_dir.join("MERGE_MSG"))
            .ok()
            .map(|text| {
                let kept: Vec<&str> = text.lines().filter(|line| !line.starts_with('#')).collect();
                kept.join("\n").trim().to_string()
            })
            .filter(|message| !message.is_empty());
    }
    Ok(status)
}

/// A merge, rebase, cherry-pick or revert under way.
fn state(git_dir: &Path) -> Option<&'static str> {
    if git_dir.join("rebase-merge").is_dir() || git_dir.join("rebase-apply").is_dir() {
        Some("rebasing")
    } else if git_dir.join("MERGE_HEAD").is_file() {
        Some("merging")
    } else if git_dir.join("CHERRY_PICK_HEAD").is_file() {
        Some("cherry-picking")
    } else if git_dir.join("REVERT_HEAD").is_file() {
        Some("reverting")
    } else {
        None
    }
}

/// Reads `git status --porcelain=v2 --branch -z`.
fn parse_status(text: &str) -> GitStatus {
    let mut status = GitStatus::default();
    let mut items = text.split('\0');
    let letter = |c: char| (c != '.').then(|| c.to_string());
    while let Some(item) = items.next() {
        if let Some(header) = item.strip_prefix("# ") {
            let (key, value) = header.split_once(' ').unwrap_or((header, ""));
            match key {
                "branch.oid" if value != "(initial)" => status.head = Some(value.to_string()),
                "branch.head" if value != "(detached)" => status.branch = Some(value.to_string()),
                "branch.upstream" => status.upstream = Some(value.to_string()),
                "branch.ab" => {
                    for part in value.split(' ') {
                        if let Some(n) = part.strip_prefix('+') {
                            status.ahead = n.parse().unwrap_or(0);
                        } else if let Some(n) = part.strip_prefix('-') {
                            status.behind = n.parse().unwrap_or(0);
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        let mut file = match item.split_at_checked(2) {
            Some(("1 ", rest)) => {
                let fields: Vec<&str> = rest.splitn(8, ' ').collect();
                let [xy, .., path] = fields.as_slice() else {
                    continue;
                };
                changed(xy, path, &letter)
            }
            Some(("2 ", rest)) => {
                let fields: Vec<&str> = rest.splitn(9, ' ').collect();
                let [xy, .., path] = fields.as_slice() else {
                    continue;
                };
                let mut file = changed(xy, path, &letter);
                file.from = items.next().map(str::to_string);
                file
            }
            Some(("u ", rest)) => {
                let fields: Vec<&str> = rest.splitn(10, ' ').collect();
                let Some(path) = fields.last() else {
                    continue;
                };
                GitFile {
                    path: path.to_string(),
                    conflict: true,
                    unstaged: Some("U".into()),
                    ..GitFile::default()
                }
            }
            Some(("? ", path)) => GitFile {
                path: path.to_string(),
                unstaged: Some("?".into()),
                ..GitFile::default()
            },
            _ => continue,
        };
        if status.files.len() >= MAX_FILES {
            status.more = true;
            continue;
        }
        file.path.shrink_to_fit();
        status.files.push(file);
    }
    status.files.sort_by(|a, b| a.path.cmp(&b.path));
    status
}

fn changed(xy: &str, path: &str, letter: &impl Fn(char) -> Option<String>) -> GitFile {
    let mut chars = xy.chars();
    GitFile {
        path: path.to_string(),
        staged: chars.next().and_then(letter),
        unstaged: chars.next().and_then(letter),
        ..GitFile::default()
    }
}

/// Reads `git diff --numstat -z`: lines added and removed by path (a renamed
/// file's new path).
fn parse_numstat(text: &str) -> HashMap<String, GitLines> {
    let mut lines = HashMap::new();
    let mut items = text.split('\0');
    while let Some(item) = items.next() {
        let mut fields = item.splitn(3, '\t');
        let (Some(added), Some(removed), Some(path)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let path = if path.is_empty() {
            // A rename: its old and new paths follow.
            items.next();
            match items.next() {
                Some(path) => path,
                None => break,
            }
        } else {
            path
        };
        let binary = added == "-";
        lines.insert(
            path.to_string(),
            GitLines {
                added: added.parse().unwrap_or(0),
                removed: removed.parse().unwrap_or(0),
                binary,
            },
        );
    }
    lines
}

/// An untracked file's lines, when it is small enough to read; binary when
/// it has a NUL byte.
fn count_lines(path: &Path) -> Option<GitLines> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > COUNT_MAX {
        return None;
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes[..bytes.len().min(8000)].contains(&0) {
        return Some(GitLines {
            binary: true,
            ..GitLines::default()
        });
    }
    let newlines = bytes.iter().filter(|&&b| b == b'\n').count();
    let last = !bytes.is_empty() && !bytes.ends_with(b"\n");
    Some(GitLines {
        added: (newlines + usize::from(last)) as u32,
        ..GitLines::default()
    })
}

/// The field and record separators in the log's format.
const FIELD: char = '\x1f';
const TRAILER: char = '\x1d';

pub async fn log(path: &str, skip: u32, limit: Option<u32>) -> Result<GitLog> {
    let repo = need_repo(path).await?;
    let limit = limit.unwrap_or(LOG_PAGE).clamp(1, MAX_LOG);
    let born = run(
        &repo.root,
        &["rev-parse", "-q", "--verify", "HEAD"],
        None,
        READ_TIMEOUT,
    )
    .await?
    .ok();
    let format = format!(
        "--format=%H{FIELD}%P{FIELD}%an{FIELD}%ae{FIELD}%at{FIELD}%D{FIELD}%s{FIELD}\
         %(trailers:key=Co-authored-by,valueonly,separator=%x1d)"
    );
    let skip = format!("--skip={skip}");
    let count = format!("-n{}", limit + 1);
    let mut args = vec![
        "log",
        "-z",
        "--date-order",
        "--decorate=full",
        &format,
        &skip,
        &count,
        "--branches",
        "--remotes",
        "--tags",
    ];
    if born {
        args.push("HEAD");
    }
    let output = run(&repo.root, &args, None, READ_TIMEOUT).await?;
    if !output.ok() {
        // A repository with no commits yet.
        if output.stderr.contains("does not have any commits") || !born {
            return Ok(GitLog::default());
        }
        bail!("{}", output.error());
    }
    let mut commits = parse_log(&output.stdout);
    let more = commits.len() > limit as usize;
    commits.truncate(limit as usize);
    Ok(GitLog { commits, more })
}

fn parse_log(text: &str) -> Vec<GitCommit> {
    text.split('\0')
        .filter_map(|record| {
            let record = record.trim_start_matches('\n');
            let fields: Vec<&str> = record.splitn(8, FIELD).collect();
            let [sha, parents, author, email, time, refs, subject, trailers] = fields.as_slice()
            else {
                return None;
            };
            Some(GitCommit {
                sha: sha.to_string(),
                parents: parents.split_whitespace().map(str::to_string).collect(),
                author: author.to_string(),
                email: email.to_string(),
                time_ms: time.parse::<i64>().unwrap_or(0) * 1000,
                subject: subject.to_string(),
                refs: parse_refs(refs),
                coauthors: trailers
                    .split(TRAILER)
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
                    .map(str::to_string)
                    .collect(),
            })
        })
        .collect()
}

/// Reads `%D` with `--decorate=full`, like
/// `HEAD -> refs/heads/main, refs/remotes/origin/main, tag: refs/tags/v1`.
fn parse_refs(text: &str) -> Vec<GitRef> {
    let mut refs = Vec::new();
    for part in text.split(", ").map(str::trim).filter(|p| !p.is_empty()) {
        let (kind, name) = if let Some(branch) = part.strip_prefix("HEAD -> ") {
            ("head", branch.strip_prefix("refs/heads/").unwrap_or(branch))
        } else if part == "HEAD" {
            ("head", "HEAD")
        } else if let Some(tag) = part.strip_prefix("tag: ") {
            ("tag", tag.strip_prefix("refs/tags/").unwrap_or(tag))
        } else if let Some(branch) = part.strip_prefix("refs/heads/") {
            ("branch", branch)
        } else if let Some(remote) = part.strip_prefix("refs/remotes/") {
            if remote.ends_with("/HEAD") {
                continue;
            }
            ("remote", remote)
        } else {
            continue;
        };
        refs.push(GitRef {
            kind: kind.into(),
            name: name.into(),
        });
    }
    refs
}

/// A commit named by its hash, never an option or a range.
fn check_sha(sha: &str) -> Result<()> {
    if (4..=64).contains(&sha.len()) && sha.chars().all(|c| c.is_ascii_hexdigit()) {
        Ok(())
    } else {
        bail!("{sha} is not a commit")
    }
}

pub async fn show(path: &str, sha: &str) -> Result<GitCommitInfo> {
    check_sha(sha)?;
    let repo = need_repo(path).await?;
    let dir = &repo.root;
    let format = format!("--format=%H{FIELD}%P{FIELD}%an{FIELD}%ae{FIELD}%at{FIELD}%B");
    let text = git(dir, &["show", "-s", &format, sha]).await?;
    let fields: Vec<&str> = text.splitn(6, FIELD).collect();
    let [sha, parents, author, email, time, message] = fields.as_slice() else {
        bail!("cannot read commit {sha}");
    };
    let parents: Vec<String> = parents.split_whitespace().map(str::to_string).collect();
    let base: Vec<&str> = match parents.first() {
        Some(parent) => vec![parent.as_str(), sha],
        None => vec!["--root", sha],
    };
    let with = |what: &'static str| {
        let mut args = vec!["diff-tree", "-r", "-z", "-M", "--no-commit-id", what];
        args.extend(&base);
        args
    };
    let (names, numbers) = (with("--name-status"), with("--numstat"));
    let (names, numbers) = tokio::join!(git(dir, &names), git(dir, &numbers));
    let numbers = parse_numstat(&numbers?);
    let mut files = parse_name_status(&names?);
    for file in &mut files {
        file.lines = numbers.get(&file.path).copied();
    }
    Ok(GitCommitInfo {
        sha: sha.to_string(),
        parents,
        author: author.to_string(),
        email: email.to_string(),
        time_ms: time.trim().parse::<i64>().unwrap_or(0) * 1000,
        message: message.trim_end().to_string(),
        files,
    })
}

/// Reads `git diff-tree --name-status -z`.
fn parse_name_status(text: &str) -> Vec<GitChange> {
    let mut changes = Vec::new();
    let mut items = text.split('\0').filter(|i| !i.is_empty());
    while let Some(status) = items.next() {
        let letter = status.chars().next().unwrap_or('M');
        let Some(first) = items.next() else { break };
        let (path, from) = if matches!(letter, 'R' | 'C') {
            match items.next() {
                Some(path) => (path, Some(first.to_string())),
                None => break,
            }
        } else {
            (first, None)
        };
        changes.push(GitChange {
            path: path.to_string(),
            from,
            status: letter.to_string(),
            lines: None,
        });
    }
    changes
}

pub async fn diff(path: &str, file: &str, staged: bool, sha: Option<&str>) -> Result<GitDiff> {
    let repo = need_repo(path).await?;
    let dir = &repo.root;
    let output = match sha {
        Some(sha) => {
            check_sha(sha)?;
            let parents = git(dir, &["rev-list", "--parents", "-n1", sha]).await?;
            let parent = parents.split_whitespace().nth(1).map(str::to_string);
            let mut args = vec!["diff-tree", "-p", "--no-commit-id"];
            match &parent {
                Some(parent) => args.extend([parent.as_str(), sha]),
                None => args.extend(["--root", sha]),
            }
            args.extend(["--", file]);
            git(dir, &args).await?
        }
        None if staged => git(dir, &["diff", "--cached", "--", file]).await?,
        None => {
            let text = git(dir, &["diff", "--", file]).await?;
            let untracked = text.is_empty()
                && !git(dir, &["ls-files", "-o", "--exclude-standard", "--", file])
                    .await?
                    .is_empty();
            if untracked {
                // Exit code 1 means "they differ", which a new file does.
                let output = run(
                    dir,
                    &["diff", "--no-index", "--", "/dev/null", file],
                    None,
                    READ_TIMEOUT,
                )
                .await?;
                if !matches!(output.code, Some(0 | 1)) {
                    bail!("{}", output.error());
                }
                return Ok(diff_body(file, &output.stdout));
            }
            text
        }
    };
    let mut diff = diff_body(file, &output);
    diff.lines =
        sha.is_none() && !diff.text.is_empty() && !diff.truncated && changes_lines(&output);
    Ok(diff)
}

/// Whether a diff only changes lines of a file that stays: not a new,
/// deleted, renamed or conflicted one, whose lines `git apply` can't take
/// a few at a time.
fn changes_lines(text: &str) -> bool {
    let header = text.find("\n@@").map_or(text, |i| &text[..i]);
    !header.starts_with("diff --cc")
        && !header.starts_with("diff --combined")
        && !header.lines().any(|line| {
            [
                "new file mode",
                "deleted file mode",
                "rename from",
                "copy from",
                "Binary files",
            ]
            .iter()
            .any(|what| line.starts_with(what))
        })
}

/// The diff from its first hunk, cut at [`MAX_DIFF`].
fn diff_body(file: &str, text: &str) -> GitDiff {
    let start = text
        .find("\n@@")
        .map(|i| i + 1)
        .or_else(|| text.starts_with("@@").then_some(0));
    let Some(start) = start else {
        return GitDiff {
            file: file.into(),
            binary: text.contains("Binary files"),
            ..GitDiff::default()
        };
    };
    let mut body = &text[start..];
    let truncated = body.len() > MAX_DIFF;
    if truncated {
        let mut end = MAX_DIFF;
        while !body.is_char_boundary(end) {
            end -= 1;
        }
        let end = body[..end].rfind('\n').map_or(end, |i| i + 1);
        body = &body[..end];
    }
    GitDiff {
        file: file.into(),
        text: body.to_string(),
        truncated,
        ..GitDiff::default()
    }
}

/// Applies the changes of `file` on the `lines` of `diff` (and no others),
/// `how` saying where to, as `git add -p` would.
async fn apply_lines(
    dir: &Path,
    file: &str,
    diff: &str,
    lines: &[usize],
    way: Way,
    how: &[&str],
) -> Result<String> {
    check_files(&[file.to_string()])?;
    let patch = patch::partial(file, diff, &lines.iter().copied().collect(), way)?;
    let mut args = vec!["apply", "--whitespace=nowarn"];
    args.extend(how);
    args.push("-");
    change(dir, &args, Some(&patch)).await.map_err(|err| {
        if err.to_string().contains("patch does not apply") {
            anyhow!("{file} changed since its diff was read; look at the diff again")
        } else {
            err
        }
    })
}

/// A file name for git, never taken as an option.
fn check_files(files: &[String]) -> Result<()> {
    if files.iter().any(|f| f.is_empty() || f.starts_with('-')) {
        bail!("not a file name: {files:?}");
    }
    Ok(())
}

fn check_branch(name: &str) -> Result<()> {
    if name.is_empty() || name.starts_with('-') || name.chars().any(char::is_whitespace) {
        bail!("{name:?} can't be a branch name");
    }
    Ok(())
}

pub async fn run_action(path: &str, action: GitAction) -> Result<GitDone> {
    if let GitAction::Init = action {
        let dir = files::resolve(path)?;
        if repo(path).await?.is_some() {
            bail!("{} is already in a Git repository", dir.display());
        }
        let output = change(&dir, &["init", "-q"], None).await?;
        return Ok(GitDone { output });
    }
    if let GitAction::SetUser { name, email } = &action {
        let (name, email) = (name.trim(), email.trim());
        if name.is_empty() || !email.contains('@') {
            bail!("a commit needs a name and an email address");
        }
        let dir = files::resolve(path)?;
        change(&dir, &["config", "--global", "user.name", name], None).await?;
        change(&dir, &["config", "--global", "user.email", email], None).await?;
        return Ok(GitDone::default());
    }
    let repo = need_repo(path).await?;
    let dir = &repo.root;
    let born = run(
        dir,
        &["rev-parse", "-q", "--verify", "HEAD"],
        None,
        READ_TIMEOUT,
    )
    .await?
    .ok();
    let output = match action {
        GitAction::Init | GitAction::SetUser { .. } => unreachable!(),
        GitAction::Stage { files } => {
            check_files(&files)?;
            let mut args = vec!["add", "-A", "--"];
            args.extend(files.iter().map(String::as_str));
            change(dir, &args, None).await?
        }
        GitAction::Unstage { files } => {
            check_files(&files)?;
            let all = [".".to_string()];
            let files = if files.is_empty() {
                &all[..]
            } else {
                &files[..]
            };
            let mut args = if born {
                vec!["reset", "-q", "--"]
            } else {
                // Nothing to reset to before the first commit.
                vec!["rm", "--cached", "-r", "-q", "--ignore-unmatch", "--"]
            };
            args.extend(files.iter().map(String::as_str));
            change(dir, &args, None).await?
        }
        GitAction::Discard { files } => {
            check_files(&files)?;
            if files.is_empty() {
                bail!("name the files to throw away");
            }
            let mut args = vec!["ls-files", "-z", "--"];
            args.extend(files.iter().map(String::as_str));
            let known: HashSet<String> = git(dir, &args)
                .await?
                .split('\0')
                .map(str::to_string)
                .collect();
            let (tracked, untracked): (Vec<&String>, Vec<&String>) = files.iter().partition(|f| {
                known.contains(f.as_str())
                    || known
                        .iter()
                        .any(|k| k.starts_with(f.as_str()) && f.ends_with('/'))
            });
            let mut said = String::new();
            if !tracked.is_empty() {
                let mut args = vec!["checkout", "-q", "--"];
                args.extend(tracked.iter().map(|f| f.as_str()));
                said.push_str(&change(dir, &args, None).await?);
            }
            if !untracked.is_empty() {
                let mut args = vec!["clean", "-f", "-d", "-q", "--"];
                args.extend(untracked.iter().map(|f| f.as_str()));
                said.push_str(&change(dir, &args, None).await?);
            }
            said
        }
        GitAction::StageLines { file, diff, lines } => {
            apply_lines(dir, &file, &diff, &lines, Way::Forward, &["--cached"]).await?
        }
        GitAction::UnstageLines { file, diff, lines } => {
            apply_lines(
                dir,
                &file,
                &diff,
                &lines,
                Way::Reverse,
                &["--cached", "--reverse"],
            )
            .await?
        }
        GitAction::DiscardLines { file, diff, lines } => {
            apply_lines(dir, &file, &diff, &lines, Way::Reverse, &["--reverse"]).await?
        }
        GitAction::Commit { message, amend } => {
            if message.trim().is_empty() {
                bail!("write a commit message first");
            }
            let mut args = vec!["commit", "-q", "-F", "-"];
            if amend {
                args.push("--amend");
            }
            change(dir, &args, Some(&message)).await?
        }
        GitAction::Fetch => change(dir, &["fetch", "--all", "--prune"], None).await?,
        GitAction::Pull => {
            // Merge unless the user chose otherwise: without a choice, a
            // newer git refuses to pull branches that both moved.
            let chosen = run(
                dir,
                &["config", "--get-regexp", r"^pull\.(rebase|ff)$"],
                None,
                READ_TIMEOUT,
            )
            .await?;
            let mut args = vec!["pull"];
            if chosen.stdout.trim().is_empty() {
                args.push("--no-rebase");
            }
            change(dir, &args, None).await?
        }
        GitAction::Push => {
            let status = status(path).await?;
            let Some(branch) = status.branch else {
                bail!("HEAD isn't on a branch; switch to one or make one to push");
            };
            if status.upstream.is_some() {
                change(dir, &["push"], None).await?
            } else {
                let remote = status
                    .remotes
                    .iter()
                    .find(|r| *r == "origin")
                    .or(status.remotes.first())
                    .context(
                        "this repository has no remote to push to; add one with \
                         `git remote add origin <url>` in a terminal",
                    )?;
                change(dir, &["push", "-u", remote, &branch], None).await?
            }
        }
        GitAction::UndoCommit => {
            if !born {
                bail!("there is no commit to undo");
            }
            let parent = run(
                dir,
                &["rev-parse", "-q", "--verify", "HEAD~1"],
                None,
                READ_TIMEOUT,
            )
            .await?
            .ok();
            if parent {
                change(dir, &["reset", "--soft", "HEAD~1"], None).await?
            } else {
                // The first commit: the branch goes back to having none,
                // with its files still staged.
                change(dir, &["update-ref", "-d", "HEAD"], None).await?
            }
        }
        GitAction::Switch { branch } => {
            check_branch(&branch)?;
            let status = status(path).await?;
            if status.branches.contains(&branch) {
                change(dir, &["checkout", "-q", &branch], None).await?
            } else if status.remote_branches.contains(&branch) {
                let local = branch.split_once('/').map_or(branch.as_str(), |(_, b)| b);
                if status.branches.iter().any(|b| b == local) {
                    change(dir, &["checkout", "-q", local], None).await?
                } else {
                    change(dir, &["checkout", "-q", "--track", &branch], None).await?
                }
            } else {
                bail!("there is no branch {branch}");
            }
        }
        GitAction::CreateBranch { name } => {
            check_branch(&name)?;
            git(dir, &["check-ref-format", "--branch", &name])
                .await
                .map_err(|_| anyhow!("{name:?} can't be a branch name"))?;
            change(dir, &["checkout", "-q", "-b", &name], None).await?
        }
        GitAction::Stash { message } => {
            let mut args = vec!["stash", "push", "-u", "-q"];
            if let Some(message) = message.as_deref().filter(|m| !m.trim().is_empty()) {
                args.extend(["-m", message]);
            }
            change(dir, &args, None).await?
        }
        GitAction::StashPop => change(dir, &["stash", "pop", "-q"], None).await?,
        GitAction::Merge { branch } => {
            check_branch(&branch)?;
            let status = status(path).await?;
            if status.branch.as_deref() == Some(branch.as_str()) {
                bail!("{branch} is the branch checked out");
            }
            if !status.branches.contains(&branch) && !status.remote_branches.contains(&branch) {
                bail!("there is no branch {branch}");
            }
            let output = run(dir, &["merge", "--no-edit", &branch], None, WRITE_TIMEOUT).await?;
            // Stopping on conflicts leaves git merging, which the pane shows.
            if !output.ok() && state(&repo.git_dir) != Some("merging") {
                bail!("{}", output.error());
            }
            output.stdout.trim().to_string()
        }
        GitAction::Resolve { file, side } => {
            check_files(std::slice::from_ref(&file))?;
            let flag = match side {
                GitSide::Ours => "--ours",
                GitSide::Theirs => "--theirs",
            };
            let taken = run(dir, &["checkout", flag, "--", &file], None, WRITE_TIMEOUT).await?;
            if taken.ok() {
                change(dir, &["add", "--", &file], None).await?
            } else if taken.stderr.contains("does not have") {
                // That side deleted the file.
                change(dir, &["rm", "-q", "--", &file], None).await?
            } else {
                bail!("{}", taken.error());
            }
        }
        GitAction::Abort => {
            let command = match state(&repo.git_dir) {
                Some("rebasing") => "rebase",
                Some("merging") => "merge",
                Some("cherry-picking") => "cherry-pick",
                Some("reverting") => "revert",
                _ => bail!("there is no merge or rebase to stop"),
            };
            change(dir, &[command, "--abort"], None).await?
        }
    };
    Ok(GitDone { output })
}

/// What the agent is asked; the staged changes follow on its input.
const MESSAGE_PROMPT: &str = "Write a git commit message for the staged changes given below. \
Reply with the message only, no code fences or commentary: a summary line of at most 60 \
characters in the imperative mood (\"Add QC plots\", not \"Added\"), then, if it helps, a blank \
line and a few short lines on what changed and why. Match the style of the recent commit \
subjects. Don't add co-author or attribution lines.";

pub async fn write_message(path: &str, agent: Option<&str>) -> Result<GitMessage> {
    let repo = need_repo(path).await?;
    let dir = &repo.root;
    let (stat, patch, recent) = tokio::join!(
        git(dir, &["diff", "--cached", "--stat=100"]),
        git(dir, &["diff", "--cached", "-U2"]),
        run(dir, &["log", "-n12", "--format=%s"], None, READ_TIMEOUT),
    );
    let stat = stat?;
    if stat.trim().is_empty() {
        bail!("stage the changes to describe first");
    }
    let mut input = String::new();
    if let Ok(recent) = recent
        && recent.ok()
        && !recent.stdout.trim().is_empty()
    {
        input.push_str("Recent commit subjects:\n");
        input.push_str(&recent.stdout);
        input.push('\n');
    }
    input.push_str("Staged changes:\n");
    input.push_str(&stat);
    input.push_str("\nThe diff");
    let patch = shorten_patch(&patch?);
    if patch.1 {
        input.push_str(" (long files cut short)");
    }
    input.push_str(":\n");
    input.push_str(&patch.0);

    let pick = |kind: &str| crate::agents::find(kind).map(|p| (kind.to_string(), p));
    let (kind, program) = match agent {
        Some(kind) => {
            pick(kind).with_context(|| format!("{kind} is not installed on this machine"))?
        }
        None => pick(crate::agents::CLAUDE)
            .or_else(|| pick(crate::agents::CODEX))
            .context(
                "Write it asks Claude Code or Codex, and neither is installed on this machine",
            )?,
    };
    let text = match kind.as_str() {
        crate::agents::CLAUDE => ask_claude(dir, &program, &input).await?,
        crate::agents::CODEX => ask_codex(dir, &program, &input).await?,
        _ => bail!("Write it can't ask {kind}"),
    };
    let (summary, body) = split_message(&text);
    if summary.is_empty() {
        bail!("{kind} didn't write a message");
    }
    Ok(GitMessage {
        summary,
        body,
        agent: kind,
    })
}

/// Each file's diff cut to [`PROMPT_FILE`] bytes, and all to
/// [`PROMPT_TOTAL`]; whether any was cut.
fn shorten_patch(patch: &str) -> (String, bool) {
    let mut out = String::new();
    let mut cut = false;
    for (i, file) in patch.split("\ndiff --git ").enumerate() {
        let file = if i == 0 {
            file.to_string()
        } else {
            format!("\ndiff --git {file}")
        };
        let take = if file.len() > PROMPT_FILE {
            cut = true;
            let mut end = PROMPT_FILE;
            while !file.is_char_boundary(end) {
                end -= 1;
            }
            format!("{}\n[…]\n", &file[..end])
        } else {
            file
        };
        if out.len() + take.len() > PROMPT_TOTAL {
            out.push_str("\n[…more files]\n");
            return (out, true);
        }
        out.push_str(&take);
    }
    (out, cut)
}

/// Runs an agent CLI with `input` on its standard input.
async fn ask(dir: &Path, program: &Path, args: &[String], input: &str) -> Result<String> {
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(path) = crate::agents::path_for(program)? {
        command.env("PATH", path);
    }
    let mut child = command
        .spawn()
        .with_context(|| format!("cannot run {}", program.display()))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(input.as_bytes()).await?;
    }
    let output = tokio::time::timeout(AGENT_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| anyhow!("the agent took too long to write a message"))??;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        let out = String::from_utf8_lossy(&output.stdout);
        let said = if err.trim().is_empty() { out } else { err };
        let said = said.trim();
        let said = said.lines().rev().take(4).collect::<Vec<_>>();
        bail!(
            "the agent couldn't write a message: {}",
            said.into_iter().rev().collect::<Vec<_>>().join("\n")
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

async fn ask_claude(dir: &Path, program: &Path, input: &str) -> Result<String> {
    let help = crate::agents::help_text(program).await;
    let mut args: Vec<String> = vec!["-p".into(), MESSAGE_PROMPT.into()];
    // A small, quick model; no tools, MCP servers or saved session.
    for (flag, value) in [
        ("--model", Some("haiku")),
        ("--tools", Some("")),
        ("--strict-mcp-config", None),
        ("--no-session-persistence", None),
        ("--disable-slash-commands", None),
    ] {
        if help.contains(flag) {
            args.push(flag.into());
            args.extend(value.map(str::to_string));
        }
    }
    ask(dir, program, &args, input).await
}

async fn ask_codex(dir: &Path, program: &Path, input: &str) -> Result<String> {
    let help = crate::agents::help_text(program).await;
    if !help.contains("exec") {
        bail!("this Codex is too old to write a message");
    }
    let out = std::env::temp_dir().join(format!("sandhill-message-{}.txt", uuid::Uuid::new_v4()));
    let out_arg = out.to_string_lossy().into_owned();
    let args: Vec<String> = [
        "exec",
        "--skip-git-repo-check",
        "--sandbox",
        "read-only",
        "--output-last-message",
        &out_arg,
        "-",
    ]
    .map(str::to_string)
    .to_vec();
    let prompt = format!("{MESSAGE_PROMPT}\n\n{input}");
    let result = ask(dir, program, &args, &prompt).await;
    let text = std::fs::read_to_string(&out);
    let _ = std::fs::remove_file(&out);
    result?;
    text.context("Codex didn't write a message")
}

/// The summary line and the body of an agent's message, without code fences
/// or attribution lines.
fn split_message(text: &str) -> (String, String) {
    let lines: Vec<&str> = text
        .trim()
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.trim_start().starts_with("```"))
        .filter(|l| {
            let lower = l.to_ascii_lowercase();
            !lower.starts_with("co-authored-by:") && !lower.contains("generated with")
        })
        .collect();
    let mut lines = lines.into_iter().skip_while(|l| l.trim().is_empty());
    let summary = lines
        .next()
        .unwrap_or("")
        .trim()
        .trim_matches('"')
        .to_string();
    let body: Vec<&str> = lines.collect();
    let body = body.join("\n").trim().to_string();
    (summary, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_status() {
        let text = "# branch.oid 1234abcd\0# branch.head main\0# branch.upstream origin/main\0\
# branch.ab +3 -1\0\
1 M. N... 100644 100644 100644 aaa bbb analysis.qmd\0\
1 .M N... 100644 100644 100644 aaa bbb R/01 qc.R\0\
2 R. N... 100644 100644 100644 aaa bbb R100 new.R\0old.R\0\
u UU N... 100644 100644 100644 100644 a b c conflict.R\0\
? CLAUDE.md\0? data/\0";
        let status = parse_status(text);
        assert_eq!(status.head.as_deref(), Some("1234abcd"));
        assert_eq!(status.branch.as_deref(), Some("main"));
        assert_eq!(status.upstream.as_deref(), Some("origin/main"));
        assert_eq!((status.ahead, status.behind), (3, 1));
        let summary: Vec<_> = status
            .files
            .iter()
            .map(|f| {
                (
                    f.path.as_str(),
                    f.staged.as_deref(),
                    f.unstaged.as_deref(),
                    f.from.as_deref(),
                    f.conflict,
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                ("CLAUDE.md", None, Some("?"), None, false),
                ("R/01 qc.R", None, Some("M"), None, false),
                ("analysis.qmd", Some("M"), None, None, false),
                ("conflict.R", None, Some("U"), None, true),
                ("data/", None, Some("?"), None, false),
                ("new.R", Some("R"), None, Some("old.R"), false),
            ]
        );
    }

    #[test]
    fn reads_a_new_repository_and_a_detached_head() {
        let status = parse_status("# branch.oid (initial)\0# branch.head main\0? a.R\0");
        assert_eq!(status.head, None);
        assert_eq!(status.branch.as_deref(), Some("main"));
        let status = parse_status("# branch.oid abc\0# branch.head (detached)\0");
        assert_eq!(status.branch, None);
        assert_eq!(status.head.as_deref(), Some("abc"));
    }

    #[test]
    fn reads_numstat_with_renames_and_binaries() {
        let lines = parse_numstat("12\t0\tanalysis.qmd\0-\t-\tumap.png\x003\t1\t\0old.R\0new.R\0");
        assert_eq!(lines["analysis.qmd"].added, 12);
        assert!(lines["umap.png"].binary);
        assert_eq!((lines["new.R"].added, lines["new.R"].removed), (3, 1));
        assert!(!lines.contains_key("old.R"));
    }

    #[test]
    fn reads_the_log_and_its_refs() {
        let text = format!(
            "aaa{FIELD}bbb ccc{FIELD}Huan Yang{FIELD}huan@example.org{FIELD}1760000000{FIELD}\
HEAD -> refs/heads/main, refs/remotes/origin/main, refs/remotes/origin/HEAD, tag: refs/tags/v0.1.0\
{FIELD}Merge branch 'feature/deseq'{FIELD}Claude <noreply@anthropic.com>{TRAILER}Codex <codex@openai.com>\
\0\nbbb{FIELD}{FIELD}Huan Yang{FIELD}huan@example.org{FIELD}1750000000{FIELD}{FIELD}First{FIELD}"
        );
        let commits = parse_log(&text);
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].parents, ["bbb", "ccc"]);
        assert_eq!(commits[0].time_ms, 1_760_000_000_000);
        let refs: Vec<_> = commits[0]
            .refs
            .iter()
            .map(|r| (r.kind.as_str(), r.name.as_str()))
            .collect();
        assert_eq!(
            refs,
            [
                ("head", "main"),
                ("remote", "origin/main"),
                ("tag", "v0.1.0")
            ]
        );
        assert_eq!(
            commits[0].coauthors,
            ["Claude <noreply@anthropic.com>", "Codex <codex@openai.com>"]
        );
        assert!(commits[1].parents.is_empty());
        assert!(commits[1].coauthors.is_empty());
        assert_eq!(parse_refs("HEAD, refs/heads/x")[0].name, "HEAD");
    }

    #[test]
    fn reads_name_status() {
        let changes = parse_name_status("M\0a.R\0R087\0old.R\0new.R\0A\0b.R\0");
        let summary: Vec<_> = changes
            .iter()
            .map(|c| (c.status.as_str(), c.path.as_str(), c.from.as_deref()))
            .collect();
        assert_eq!(
            summary,
            [
                ("M", "a.R", None),
                ("R", "new.R", Some("old.R")),
                ("A", "b.R", None)
            ]
        );
    }

    #[test]
    fn keeps_the_diff_from_its_first_hunk() {
        let diff = diff_body(
            "a.R",
            "diff --git a/a.R b/a.R\nindex 1..2 100644\n--- a/a.R\n+++ b/a.R\n@@ -1 +1 @@\n-x\n+y\n",
        );
        assert_eq!(diff.text, "@@ -1 +1 @@\n-x\n+y\n");
        let binary = diff_body("a.png", "diff --git a/a.png b/a.png\nBinary files differ\n");
        assert!(binary.binary);
        assert!(binary.text.is_empty());
    }

    #[test]
    fn splits_an_agents_message() {
        let (summary, body) = split_message(
            "```\nAdd QC violin plots\n\nPlots nCount and percent.mt per sample.\n\nCo-Authored-By: Claude <noreply@anthropic.com>\n```\n",
        );
        assert_eq!(summary, "Add QC violin plots");
        assert_eq!(body, "Plots nCount and percent.mt per sample.");
        assert_eq!(split_message("\"Fix typo\"").0, "Fix typo");
    }

    #[test]
    fn cuts_long_files_from_the_prompt() {
        let long = format!("diff --git a/a b/a\n{}", "+x\n".repeat(5000));
        let patch = format!("{long}\ndiff --git a/b b/b\n+y\n");
        let (text, cut) = shorten_patch(&patch);
        assert!(cut);
        assert!(text.contains("[…]"));
        assert!(text.contains("diff --git a/b b/b\n+y"));
    }

    #[test]
    fn refuses_options_for_names() {
        assert!(check_sha("abc123").is_ok());
        assert!(check_sha("--output=x").is_err());
        assert!(check_files(&["-rf".into()]).is_err());
        assert!(check_branch("--force").is_err());
        assert!(check_branch("feature/deseq").is_ok());
    }
}
