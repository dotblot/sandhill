//! Ghost cells: after the user runs a notebook cell, a coding agent
//! suggests the cell to come next, which the notebook shows faded until the
//! user takes or dismisses it. The agent is one the user already has
//! signed in, run once as its own unmodified CLI (`claude -p`, `codex
//! exec`) in the project's folder, with no tools: it only reads the cells
//! it is given and answers with code. Nothing is saved as a conversation.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use tokio::io::AsyncWriteExt;

use crate::agents::{self, CLAUDE, CODEX};

/// How long an agent may take to answer.
const TIMEOUT: Duration = Duration::from_secs(120);
/// How much of each cell, and of the output, the agent is shown.
const MAX_CELL: usize = 3000;
const MAX_OUTPUT: usize = 3000;
/// How many cells, the last ones, the agent is shown.
const MAX_CELLS: usize = 12;
/// Claude Code's model for suggestions: quick, as one comes after each run.
const CLAUDE_MODEL: &str = "haiku";

/// The agent to ask: `wanted` when it can answer once from the command
/// line and is installed, else Claude Code, else Codex.
pub fn choose(wanted: Option<&str>) -> Result<(&'static str, PathBuf)> {
    let order = [CLAUDE, CODEX];
    let first = order.iter().copied().find(|k| Some(*k) == wanted);
    for kind in first.into_iter().chain(order) {
        if let Some(path) = agents::find(kind) {
            return Ok((kind, path));
        }
    }
    bail!("suggesting cells needs Claude Code or Codex on this machine")
}

/// What the agent is asked.
pub fn prompt(language: &str, cells: &[String], output: &str) -> String {
    let fence = fence_language(language);
    let mut text = format!(
        "You suggest the next cell of a {language} notebook for data analysis, as an assistant \
         looking over the analyst's shoulder would. Below are its last cells, oldest first, and \
         what the last one, just run, showed. Reply with only the next cell's code in one \
         ```{fence} block: one small, useful next step that follows from what they are doing, \
         at most about 15 lines, using the names already defined. Don't repeat a cell, don't \
         install packages, and write nothing outside the block. If there is no sensible next \
         step, reply with just NONE.\n"
    );
    let skip = cells.len().saturating_sub(MAX_CELLS);
    for (i, cell) in cells.iter().enumerate().skip(skip) {
        text.push_str(&format!(
            "\nCell {}:\n```{fence}\n{}\n```\n",
            i + 1,
            clip(cell.trim_end(), MAX_CELL)
        ));
    }
    let output = output.trim();
    if output.is_empty() {
        text.push_str("\nThe last cell showed nothing.\n");
    } else {
        text.push_str(&format!(
            "\nThe last cell showed:\n```\n{}\n```\n",
            clip(output, MAX_OUTPUT)
        ));
    }
    text
}

fn fence_language(language: &str) -> &'static str {
    match language.to_ascii_lowercase().as_str() {
        "r" => "r",
        "python" | "python3" => "python",
        "julia" => "julia",
        _ => "",
    }
}

fn clip(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n… (cut)", &text[..end])
}

/// The code in the agent's reply: its first fenced block, or the whole
/// reply when it wrote no fence. None when it saw no next step.
pub fn parse(reply: &str) -> Option<String> {
    let reply = reply.trim();
    if reply.is_empty()
        || reply
            .trim_matches(['*', '`', '.'])
            .eq_ignore_ascii_case("none")
    {
        return None;
    }
    let code = match reply.find("```") {
        Some(start) => {
            let rest = &reply[start + 3..];
            // The fence's language, if any, runs to the end of its line.
            let body = rest.split_once('\n').map_or("", |(_, b)| b);
            body.find("```").map_or(body, |end| &body[..end])
        }
        None => reply,
    };
    let code = code.trim_matches('\n').trim_end();
    (!code.trim().is_empty()).then(|| code.to_string())
}

/// Asks the agent `kind` at `program` once, in `cwd`, and returns its reply.
pub async fn ask(kind: &str, program: &Path, cwd: &Path, prompt: &str) -> Result<String> {
    let help = agents::help_text(program).await;
    let has = |flag: &str| help.contains(flag);
    let answer_file = tempfile_path();
    let args: Vec<String> = match kind {
        CLAUDE => {
            let mut args: Vec<String> = ["-p", "--output-format", "text", "--model", CLAUDE_MODEL]
                .map(String::from)
                .into();
            if has("--tools") {
                args.extend(["--tools".into(), String::new()]);
            }
            for flag in [
                "--strict-mcp-config",
                "--no-session-persistence",
                "--disable-slash-commands",
            ] {
                if has(flag) {
                    args.push(flag.into());
                }
            }
            args
        }
        CODEX => vec![
            "exec".into(),
            "--skip-git-repo-check".into(),
            "--sandbox".into(),
            "read-only".into(),
            "--color".into(),
            "never".into(),
            "--output-last-message".into(),
            answer_file.to_string_lossy().into_owned(),
            "-".into(),
        ],
        _ => bail!("{kind} can't suggest cells"),
    };
    let mut child = tokio::process::Command::new(program)
        .args(&args)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("cannot start {}", program.display()))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(prompt.as_bytes()).await?;
    }
    let output = tokio::time::timeout(TIMEOUT, child.wait_with_output())
        .await
        .context("the agent took too long to suggest a cell")??;
    let answer = std::fs::read_to_string(&answer_file).ok();
    let _ = std::fs::remove_file(&answer_file);
    if !output.status.success() {
        let said = String::from_utf8_lossy(&output.stderr);
        let said = said
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let said = if said.is_empty() {
            stdout.trim()
        } else {
            said.trim()
        };
        bail!("{} failed: {}", program.display(), clip(said, 300));
    }
    Ok(answer
        .filter(|a| !a.trim().is_empty())
        .unwrap_or_else(|| String::from_utf8_lossy(&output.stdout).into_owned()))
}

/// Where Codex writes its last message.
fn tempfile_path() -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "sandhill-ghost-{}-{}.txt",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asks_with_the_last_cells_and_reads_the_code() {
        let cells: Vec<String> = (1..=14).map(|i| format!("x{i} = {i}")).collect();
        let text = prompt("python", &cells, "14\n");
        assert!(!text.contains("x2 = 2") && text.contains("Cell 3:\n```python\nx3 = 3\n```"));
        assert!(text.contains("The last cell showed:\n```\n14\n```"));
        assert!(prompt("R", &[], "").contains("showed nothing"));

        assert_eq!(
            parse("Here:\n```python\nadata.obs.head()\n```\nDone").as_deref(),
            Some("adata.obs.head()")
        );
        assert_eq!(
            parse("```\nsummary(df)\n```").as_deref(),
            Some("summary(df)")
        );
        assert_eq!(parse("plot(x)\n").as_deref(), Some("plot(x)"));
        assert_eq!(parse("NONE"), None);
        assert_eq!(parse("**None**"), None);
        assert_eq!(parse("```python\n```"), None);
        assert_eq!(clip("ééé", 3), "é\n… (cut)");
    }

    #[tokio::test]
    async fn runs_the_agent_once_with_no_tools() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("claude");
        // Says which flags it got and what it was asked, as a cell.
        std::fs::write(
            &fake,
            "#!/bin/sh\nif [ \"$1\" = --help ]; then echo '--tools --no-session-persistence'; exit 0; fi\nasked=$(cat)\necho '```python'\necho \"# $*\"\necho \"# $(echo \"$asked\" | grep -c 'Cell 1')\"\necho '```'\n",
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let reply = ask(
            CLAUDE,
            &fake,
            dir.path(),
            &prompt("python", &["a = 1".into()], ""),
        )
        .await
        .unwrap();
        let code = parse(&reply).unwrap();
        assert!(
            code.contains(
                "-p --output-format text --model haiku --tools  --no-session-persistence"
            ),
            "{code}"
        );
        assert!(code.ends_with("# 1"), "{code}");
        assert!(ask("antigravity", &fake, dir.path(), "x").await.is_err());
    }
}
