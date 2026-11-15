use super::*;

/// Applies the changes of `file` on the `lines` of `diff` (and no others),
/// `how` saying where to, as `git add -p` would.
pub(super) async fn apply_lines(
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
pub(super) fn check_files(files: &[String]) -> Result<()> {
    if files.iter().any(|f| f.is_empty() || f.starts_with('-')) {
        bail!("not a file name: {files:?}");
    }
    Ok(())
}

/// Runs a merge, rebase or pull that may stop on conflicts: stopping leaves
/// git merging or rebasing, which the pane shows, and isn't a failure.
pub(super) async fn stops(repo: &Repo, args: &[&str]) -> Result<String> {
    let output = run(&repo.root, args, None, WRITE_TIMEOUT).await?;
    if !output.ok() && state(&repo.git_dir).is_none() {
        bail!("{}", output.error());
    }
    let mut said = output.stdout.trim().to_string();
    let err = output.stderr.trim();
    if !output.ok() && !err.is_empty() {
        if !said.is_empty() {
            said.push('\n');
        }
        said.push_str(err);
    }
    Ok(said)
}

pub(super) async fn check_tag(dir: &Path, name: &str) -> Result<()> {
    if name.is_empty() || name.starts_with('-') {
        bail!("{name:?} can't be a tag name");
    }
    git(dir, &["check-ref-format", &format!("refs/tags/{name}")])
        .await
        .map(drop)
        .map_err(|_| anyhow!("{name:?} can't be a tag name"))
}

pub(super) fn check_branch(name: &str) -> Result<()> {
    if name.is_empty() || name.starts_with('-') || name.chars().any(char::is_whitespace) {
        bail!("{name:?} can't be a branch name");
    }
    Ok(())
}

/// Makes the branch's own commits again as `steps` says, with an interactive
/// rebase whose list of commits is written for it: `pick`, `fixup` for one
/// squashed into the one before, and after a commit given a new message, an
/// `exec` that amends it with that message.
pub(super) async fn rewrite(
    repo: &Repo,
    base: Option<String>,
    steps: &[GitRewrite],
) -> Result<String> {
    let dir = &repo.root;
    if state(&repo.git_dir).is_some() {
        bail!("finish or stop the merge or rebase under way first");
    }
    if !run(dir, &["symbolic-ref", "-q", "HEAD"], None, READ_TIMEOUT)
        .await?
        .ok()
    {
        bail!("HEAD isn't on a branch; switch to one first");
    }
    let own = own(repo).await?;
    let mut want: Vec<&str> = own.commits.iter().map(|c| c.sha.as_str()).collect();
    let mut got: Vec<&str> = steps.iter().map(|s| s.sha.as_str()).collect();
    want.sort_unstable();
    got.sort_unstable();
    if own.base != base || want != got {
        bail!("the branch's commits changed since they were listed; look again");
    }
    if steps.first().is_some_and(|s| s.squash) {
        bail!("the first commit has nothing before it to squash into");
    }
    let messages: HashMap<&str, &str> = own
        .commits
        .iter()
        .map(|c| (c.sha.as_str(), c.message.as_str()))
        .collect();
    fn changed<'a>(step: &'a GitRewrite, messages: &HashMap<&str, &str>) -> Option<&'a str> {
        step.message
            .as_deref()
            .map(str::trim_end)
            .filter(|m| Some(m) != messages.get(step.sha.as_str()))
    }
    let new_message = |step| changed(step, &messages);
    let same_order = got.len() == steps.len()
        && steps
            .iter()
            .map(|s| s.sha.as_str())
            .eq(own.commits.iter().map(|c| c.sha.as_str()));
    if same_order && steps.iter().all(|s| !s.squash && new_message(s).is_none()) {
        return Ok(String::new());
    }

    let work = repo.git_dir.join("sandhill-rewrite");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work)?;
    let program = program()?;
    let mut todo = String::new();
    for (i, step) in steps.iter().enumerate() {
        todo.push_str(&format!(
            "{} {}\n",
            if step.squash { "fixup" } else { "pick" },
            step.sha
        ));
        // A message goes on once the commits squashed into it are in.
        let start = steps[..=i].iter().rposition(|s| !s.squash).unwrap_or(0);
        let ends = steps.get(i + 1).is_none_or(|next| !next.squash);
        if let Some(message) = new_message(&steps[start]).filter(|_| ends) {
            if message.trim().is_empty() {
                bail!("a commit needs a message");
            }
            let file = work.join(format!("message-{i}.txt"));
            std::fs::write(&file, format!("{message}\n"))?;
            todo.push_str(&format!(
                "exec {} commit --amend --no-verify --allow-empty -q -F {}\n",
                shell_quote(&program.to_string_lossy()),
                shell_quote(&file.to_string_lossy())
            ));
        }
    }
    let list = work.join("todo");
    std::fs::write(&list, todo)?;
    // Git runs the sequence editor with the list's file after it.
    let editor = format!(
        "sequence.editor=cp {}",
        shell_quote(&list.to_string_lossy())
    );
    let onto = base.as_deref().unwrap_or("--root");
    let args = [
        "-c",
        "core.editor=true",
        "-c",
        &editor,
        "rebase",
        "-i",
        "--autostash",
        onto,
    ];
    let said = stops(repo, &args).await?;
    if state(&repo.git_dir).is_none() {
        let _ = std::fs::remove_dir_all(&work);
    }
    Ok(said)
}

/// `text` as one word to a POSIX shell.
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}
