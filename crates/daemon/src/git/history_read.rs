use super::*;

pub(super) fn parse_log(text: &str) -> Vec<GitCommit> {
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

/// The branch's own commits: on no remote branch, back to the last merge
/// (a rebase would flatten it), at most [`MAX_OWN`].
pub(super) async fn own(repo: &Repo) -> Result<GitOwnCommits> {
    let dir = &repo.root;
    let born = run(
        dir,
        &["rev-parse", "-q", "--verify", "HEAD"],
        None,
        READ_TIMEOUT,
    )
    .await?
    .ok();
    if !born {
        return Ok(GitOwnCommits {
            note: Some("There are no commits yet.".into()),
            ..GitOwnCommits::default()
        });
    }
    let format = format!("--format=%H{FIELD}%P{FIELD}%at{FIELD}%B{RECORD}");
    let count = format!("-n{}", MAX_OWN + 1);
    let args = [
        "log",
        "--first-parent",
        &format,
        &count,
        "HEAD",
        "--not",
        "--remotes",
    ];
    let text = git(dir, &args).await?;
    let mut commits = Vec::new();
    let mut base = None;
    let mut note = None;
    for record in text.split(RECORD) {
        let fields: Vec<&str> = record.trim_start_matches('\n').splitn(4, FIELD).collect();
        let [sha, parents, time, message] = fields.as_slice() else {
            continue;
        };
        let parents: Vec<&str> = parents.split_whitespace().collect();
        if parents.len() > 1 {
            note = Some("Older commits come before a merge, which can't be squashed or moved.");
            base = Some(sha.to_string());
            break;
        }
        if commits.len() == MAX_OWN {
            note = Some("Only the newest 50 commits can be changed here.");
            base = Some(sha.to_string());
            break;
        }
        commits.push(GitOwnCommit {
            sha: sha.to_string(),
            time_ms: time.parse::<i64>().unwrap_or(0) * 1000,
            message: message.trim_end().to_string(),
        });
        base = parents.first().map(|p| p.to_string());
    }
    if commits.is_empty() && note.is_none() {
        base = Some(git(dir, &["rev-parse", "HEAD"]).await?.trim().to_string());
        note = Some(
            "Every commit is on the remote already, so changing them would mean a forced push.",
        );
    } else if note.is_none() && base.is_some() {
        note = Some(
            "Older commits are on the remote already, so changing them would mean a forced push.",
        );
    }
    commits.reverse();
    Ok(GitOwnCommits {
        commits,
        base,
        note: note.map(str::to_string),
    })
}

/// Reads `git diff-tree --name-status -z`.
pub(super) fn parse_name_status(text: &str) -> Vec<GitChange> {
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
