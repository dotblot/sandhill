//! Git: a repository's status, log, commits, diffs and the actions run on it.

use super::*;

/// A repository's state, as [`Call::GitStatus`] asked.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct GitStatus {
    /// The repository's top folder; empty when the folder isn't in one.
    pub repo: String,
    /// The branch checked out; `None` when HEAD is detached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// HEAD's commit; `None` before the first commit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    /// The branch's upstream, like `origin/main`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream: Option<String>,
    #[serde(default)]
    pub ahead: u32,
    #[serde(default)]
    pub behind: u32,
    /// Changed, untracked and conflicted files, by path.
    #[serde(default)]
    pub files: Vec<GitFile>,
    /// Whether there were more files than `files` holds.
    #[serde(default)]
    pub more: bool,
    #[serde(default)]
    pub stashes: u32,
    /// `merging`, `rebasing`, `cherry-picking` or `reverting`, while one is
    /// under way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// While merging, the message git has ready for the merge's commit,
    /// without its comments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge_message: Option<String>,
    /// Changes whenever a branch, tag or HEAD moves, so a client knows when
    /// to read the log again.
    #[serde(default)]
    pub refs: String,
    /// Local branches, by name.
    #[serde(default)]
    pub branches: Vec<String>,
    /// Remote branches, like `origin/main`.
    #[serde(default)]
    pub remote_branches: Vec<String>,
    /// Tags, the newest first.
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub remotes: Vec<String>,
    /// `user.name` and `user.email`, which a commit needs; empty when unset.
    #[serde(default)]
    pub user_name: String,
    #[serde(default)]
    pub user_email: String,
}

/// A file in [`GitStatus`].
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct GitFile {
    /// Relative to the repository.
    pub path: String,
    /// What a renamed or copied file was called.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// The staged change: `M`odified, `A`dded, `D`eleted, `R`enamed,
    /// `C`opied or `T`ype changed; `None` when nothing is staged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub staged: Option<String>,
    /// The change not staged: `M`, `D`, `T`, or `?` for an untracked file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unstaged: Option<String>,
    /// Both sides changed it in a merge that stopped.
    #[serde(default)]
    pub conflict: bool,
    /// Lines added and removed by the staged change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub staged_lines: Option<GitLines>,
    /// Lines added and removed by the change not staged (an untracked
    /// file's lines, when it is text).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<GitLines>,
    /// Bytes, for a file that is new to the repository, so a big data file
    /// can be flagged before it is committed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct GitLines {
    pub added: u32,
    pub removed: u32,
    #[serde(default)]
    pub binary: bool,
}

/// Commits, as [`Call::GitLog`] asked.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct GitLog {
    pub commits: Vec<GitCommit>,
    /// Whether older commits follow.
    #[serde(default)]
    pub more: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct GitCommit {
    pub sha: String,
    pub parents: Vec<String>,
    pub author: String,
    pub email: String,
    pub time_ms: i64,
    pub subject: String,
    /// Branches, tags and HEAD pointing at it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refs: Vec<GitRef>,
    /// `Co-authored-by` trailers, like `Claude <noreply@anthropic.com>`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub coauthors: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct GitRef {
    /// `head` (the branch checked out, or `HEAD` when detached), `branch`,
    /// `remote` or `tag`.
    pub kind: String,
    pub name: String,
}

/// The commits [`Call::GitOwnCommits`] found, oldest first, on top of
/// `base` (none: from the repository's first commit).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct GitOwnCommits {
    pub commits: Vec<GitOwnCommit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    /// Why older commits aren't among them, or why there are none: pushed,
    /// a merge, or more than 50.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct GitOwnCommit {
    pub sha: String,
    pub time_ms: i64,
    /// The whole message.
    pub message: String,
}

/// A commit's place in a [`GitAction::Rewrite`].
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct GitRewrite {
    pub sha: String,
    /// Folds it into the commit before it, as `git rebase -i`'s fixup does.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub squash: bool,
    /// The message of the commit it makes, with the ones squashed into it;
    /// its own when none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// A commit's details, as [`Call::GitShow`] asked.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct GitCommitInfo {
    pub sha: String,
    pub parents: Vec<String>,
    pub author: String,
    pub email: String,
    pub time_ms: i64,
    /// The whole message.
    pub message: String,
    /// What it changed against its first parent.
    pub files: Vec<GitChange>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct GitChange {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// `M`, `A`, `D`, `R`, `C` or `T`.
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<GitLines>,
}

/// A file's diff, as [`Call::GitDiff`] asked.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct GitDiff {
    pub file: String,
    /// `git diff`'s unified diff, without its header lines.
    pub text: String,
    /// Whether it was cut short (at 2 MB).
    #[serde(default)]
    pub truncated: bool,
    #[serde(default)]
    pub binary: bool,
    /// Whether its lines can be staged, unstaged or thrown away one by one:
    /// a changed file git already tracks, with all of its diff here.
    #[serde(default)]
    pub lines: bool,
}

/// What [`Call::GitRun`] does.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum GitAction {
    /// Stages files (relative to the repository); every change when empty.
    Stage {
        #[serde(default)]
        files: Vec<String>,
    },
    /// Unstages files; every staged change when empty.
    Unstage {
        #[serde(default)]
        files: Vec<String>,
    },
    /// Throws away files' changes not staged; deletes untracked ones.
    Discard {
        files: Vec<String>,
    },
    /// Stages some lines of a file's changes: `lines` are places among the
    /// lines of `diff`, the text [`GitDiff`] gave (counting from 0). Git
    /// refuses it when the file is no longer as that diff showed it.
    StageLines {
        file: String,
        diff: String,
        lines: Vec<usize>,
    },
    /// Unstages some lines of a file's staged diff, as for `StageLines`.
    UnstageLines {
        file: String,
        diff: String,
        lines: Vec<usize>,
    },
    /// Throws away some lines of a file's changes not staged.
    DiscardLines {
        file: String,
        diff: String,
        lines: Vec<usize>,
    },
    /// Commits what's staged; `amend` replaces the last commit.
    Commit {
        message: String,
        #[serde(default)]
        amend: bool,
    },
    Fetch,
    /// Pulls the upstream, merging unless the user's configuration says to
    /// rebase, or with `rebase`, putting the branch's own commits on top.
    Pull {
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        rebase: bool,
    },
    /// Pushes, setting the upstream to the first remote when there is none.
    Push,
    /// Undoes the last commit, keeping its changes staged.
    UndoCommit,
    /// Switches to a branch; a remote branch (`origin/x`) gets a local
    /// branch following it.
    Switch {
        branch: String,
    },
    /// Makes a branch at HEAD and switches to it.
    CreateBranch {
        name: String,
    },
    /// Stashes every change, untracked files included.
    Stash {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    /// Brings back the last stash.
    StashPop,
    /// Merges a branch, local or remote, into the one checked out. When the
    /// two changed the same lines, git stops with those files conflicted, to
    /// settle and commit (or `Abort`); that isn't a failure.
    Merge {
        branch: String,
    },
    /// Settles a conflicted file by taking one side's version of all of it,
    /// and stages it.
    Resolve {
        file: String,
        side: GitSide,
    },
    /// Stops a merge, rebase, cherry-pick or revert that stopped on conflicts.
    Abort,
    /// Goes on with a rebase, cherry-pick or revert once its conflicts are
    /// settled and staged; it may stop again at a later commit. A merge
    /// goes on with a commit.
    Continue,
    /// Puts the branch checked out's own commits on top of `branch`, local
    /// or remote. It stops on conflicts as `Merge` does, to settle, stage
    /// and `Continue` (or `Abort`).
    Rebase {
        branch: String,
    },
    /// Makes the branch's own commits ([`Call::GitOwnCommits`], on top of
    /// `base`) again in the order of `commits`, oldest first, each folded
    /// into the one before it or with a new message as it says: an
    /// interactive rebase. Changes not committed are put aside and back. It
    /// stops on conflicts as `Rebase` does.
    Rewrite {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        base: Option<String>,
        commits: Vec<GitRewrite>,
    },
    /// Tags HEAD, or the commit `sha`, with an annotated tag.
    CreateTag {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sha: Option<String>,
    },
    DeleteTag {
        name: String,
    },
    /// Pushes a tag to the upstream's remote, or the first.
    PushTag {
        name: String,
    },
    /// Makes the folder a repository.
    Init,
    /// Sets `user.name` and `user.email` for every repository of the user.
    SetUser {
        name: String,
        email: String,
    },
}

/// A side of a conflict, for [`GitAction::Resolve`].
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GitSide {
    /// The branch checked out, or while rebasing, the branch's own commit.
    Ours,
    /// The branch being merged in, or rebased onto.
    Theirs,
}

/// What [`Call::GitRun`] did.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct GitDone {
    /// What git said, like "Everything up-to-date".
    #[serde(default)]
    pub output: String,
}

/// A commit message, as [`Call::GitWriteMessage`] asked.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct GitMessage {
    pub summary: String,
    #[serde(default)]
    pub body: String,
    /// The agent that wrote it: `claude` or `codex`.
    pub agent: String,
}
