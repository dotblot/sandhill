use super::*;

/// What an agent sees of the staged changes when writing a message.
const PROMPT_FILE: usize = 4000;
const PROMPT_TOTAL: usize = 40_000;
const AGENT_TIMEOUT: Duration = Duration::from_secs(180);
/// What the agent is asked; the staged changes follow on its input.
const MESSAGE_PROMPT: &str = "Write a git commit message for the staged changes given below. \
Reply with the message only, no code fences or commentary: a summary line of at most 60 \
characters in the imperative mood (\"Add QC plots\", not \"Added\"), then, if it helps, a blank \
line and a few short lines on what changed and why. Match the style of the recent commit \
subjects. Don't add co-author or attribution lines.";

/// Each file's diff cut to [`PROMPT_FILE`] bytes, and all to
/// [`PROMPT_TOTAL`]; whether any was cut.
pub(super) fn shorten_patch(patch: &str) -> (String, bool) {
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

pub(super) async fn ask_claude(dir: &Path, program: &Path, input: &str) -> Result<String> {
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

pub(super) async fn ask_codex(dir: &Path, program: &Path, input: &str) -> Result<String> {
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
pub(super) fn split_message(text: &str) -> (String, String) {
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
}
