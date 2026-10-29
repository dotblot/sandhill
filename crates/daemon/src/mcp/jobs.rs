//! The tools for the project's jobs, the scripts and pipelines the user
//! runs in the background: which there are (`list_jobs`), and what one
//! printed (`read_job_log`), so an agent can follow a long run or explain
//! why it failed.

use base64::Engine;
use sandhill_proto::{TERMINAL_JOB, TerminalInfo};

use super::*;

/// Lines of a log an agent gets unless it asks, and at most.
const LOG_LINES: usize = 200;
const MAX_LOG_LINES: usize = 2000;
/// The most of a log read, from its end.
const MAX_LOG_BYTES: u64 = 1 << 20;
/// How long the daemon may take to send the log.
const LOG_WAIT: Duration = Duration::from_secs(10);

impl Tools {
    /// This project's jobs, the newest first.
    async fn jobs(&self, client: &Client) -> Result<Vec<TerminalInfo>> {
        let mut jobs: Vec<TerminalInfo> = client
            .list_terminals()
            .await?
            .into_iter()
            .filter(|t| t.kind == TERMINAL_JOB)
            .filter(|t| match (&self.project, &t.project) {
                (None, _) => true,
                (Some(project), Some(p)) => p == project,
                (Some(project), None) => Path::new(&t.cwd).starts_with(project),
            })
            .collect();
        jobs.reverse();
        Ok(jobs)
    }

    pub(super) async fn list_jobs(&self) -> Result<Vec<Value>> {
        let client = self.client().await?;
        let jobs = self.jobs(&client).await?;
        if jobs.is_empty() {
            return Ok(vec![text_item("This project has no jobs.")]);
        }
        let now = now_ms();
        let lines: Vec<String> = jobs.iter().map(|j| describe_job(j, now)).collect();
        Ok(vec![text_item(&format!(
            "This project's jobs, the newest first:\n{}",
            lines.join("\n")
        ))])
    }

    pub(super) async fn read_job_log(
        &self,
        job: Option<&str>,
        lines: Option<u64>,
    ) -> Result<Vec<Value>> {
        let mut client = self.client().await?;
        let jobs = self.jobs(&client).await?;
        let found = match job {
            Some(job) => jobs
                .iter()
                .find(|j| j.id.starts_with(job) || j.name.as_deref() == Some(job) || j.title == job)
                .with_context(|| {
                    format!("this project has no job {job:?}; list_jobs lists them")
                })?,
            None => jobs.first().context("this project has no jobs")?,
        };
        let from = found.end.saturating_sub(MAX_LOG_BYTES);
        let subscribed = client.subscribe_terminal(&found.id, Some(from)).await?;
        let mut bytes = Vec::new();
        let mut at = subscribed.from;
        while at < found.end {
            let Ok(Some(event)) =
                tokio::time::timeout(LOG_WAIT, client.next_terminal_event()).await
            else {
                break;
            };
            if event.terminal != found.id {
                continue;
            }
            if let Some(data) = event.data {
                let data = base64::engine::general_purpose::STANDARD.decode(data)?;
                at = event.offset.unwrap_or(at) + data.len() as u64;
                bytes.extend(data);
            }
            if event.exit.is_some() {
                break;
            }
        }
        let _ = client.unsubscribe_terminal(&found.id).await;
        let want = lines
            .map(|n| (n as usize).clamp(1, MAX_LOG_LINES))
            .unwrap_or(LOG_LINES);
        let log = log_text(&String::from_utf8_lossy(&bytes));
        let all: Vec<&str> = log.lines().collect();
        let shown = &all[all.len().saturating_sub(want)..];
        let head = describe_job(found, now_ms());
        let text = if shown.is_empty() {
            format!("{head}\nIt has printed nothing.")
        } else if shown.len() == 1 && (all.len() > 1 || from > 0) {
            format!("{head}\nThe last line of its log:\n{}", shown[0])
        } else if shown.len() < all.len() || from > 0 {
            format!(
                "{head}\nThe last {} lines of its log:\n{}",
                shown.len(),
                shown.join("\n")
            )
        } else {
            format!("{head}\nIts log:\n{}", shown.join("\n"))
        };
        Ok(vec![text_item(&text)])
    }
}

/// "job 3f2a9c1e "align.sh" (bash align.sh lane1) · running for 12 min 3 s"
fn describe_job(job: &TerminalInfo, now: i64) -> String {
    let name = job.name.as_deref().unwrap_or(&job.title);
    let what = job
        .job
        .as_ref()
        .map(|spec| match (&spec.path, &spec.command) {
            (Some(path), Some(args)) => format!("{path} {args}"),
            (Some(path), None) => path.clone(),
            (None, Some(command)) => command.clone(),
            (None, None) => String::new(),
        })
        .unwrap_or_default();
    let state = match &job.exit {
        _ if job.running => format!("running for {}", text::duration_ms(now - job.started_ms)),
        Some(exit) => {
            let how = match (exit.code, exit.signal) {
                (Some(0), _) => "finished".to_string(),
                (Some(code), _) => format!("failed with exit code {code}"),
                (None, Some(signal)) => format!("stopped by signal {signal}"),
                (None, None) => "ended".to_string(),
            };
            format!(
                "{how} after {}, {} ago",
                text::duration_ms(exit.ended_ms - job.started_ms),
                text::duration_ms(now - exit.ended_ms)
            )
        }
        None => "not running".to_string(),
    };
    format!(
        "job {} \"{name}\" ({what}) in {} · {state}",
        short_id(&job.id),
        job.cwd
    )
}

/// A terminal's output as plain text: colours gone, and of a line a
/// progress bar redrew, only what shows last.
fn log_text(raw: &str) -> String {
    text::strip_ansi(raw)
        .split('\n')
        .map(|line| {
            let line = line.trim_end_matches('\r');
            line.rsplit('\r').next().unwrap_or(line)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_bars_show_as_they_end() {
        let raw = "start\r\n\x1b[32m10%\x1b[0m\r50%\r100%\r\ndone\r\n";
        assert_eq!(log_text(raw), "start\n100%\ndone\n");
    }
}
