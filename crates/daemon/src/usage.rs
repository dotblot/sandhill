//! What the agents in Sandhill's conversations used (Huan, 2026-11-16; Usage
//! on Welcome). Each conversation's terminal host records every model
//! request the agent reports, from what it already tells Sandhill (Claude
//! Code's status line, Codex's token counts, pi's replies, an ACP agent's
//! turns), in `usage/<year>-<month>.jsonl` under the state folder: when,
//! which agent, account, model and project, the tokens and the agent's own
//! cost estimate. Nothing is read from the agents' folders.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use anyhow::Result;
use sandhill_proto::{UsageDay, UsageRow, UsageStats, UsageTotals};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One model request, as recorded.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Record {
    pub ts_ms: i64,
    /// The agent's kind (`claude`, `codex`, `omp`…).
    pub agent: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default)]
    pub input: u64,
    #[serde(default)]
    pub cache_read: u64,
    #[serde(default)]
    pub cache_write: u64,
    #[serde(default)]
    pub output: u64,
    #[serde(default)]
    pub reasoning: u64,
    /// The agent's own estimate, in US dollars, when it gave one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
}

impl Record {
    /// Takes a request's tokens from `{input, cache_read, cache_write,
    /// output, reasoning}` (the shape the agents' reports are put in);
    /// whether it had any.
    pub fn tokens(&mut self, counts: &Value) -> bool {
        let n = |key: &str| counts[key].as_u64().unwrap_or(0);
        self.input = n("input");
        self.cache_read = n("cache_read");
        self.cache_write = n("cache_write");
        self.output = n("output");
        self.reasoning = n("reasoning");
        self.input + self.cache_read + self.cache_write + self.output + self.reasoning > 0
    }
}

/// What a new status (`status`, after `before`) says of a request: when
/// its `last` request differs from the one before, that request's counts,
/// the model, and the rise in the conversation's total cost (`cost_usd`,
/// Claude Code's; a total lower than before started again from nothing).
pub fn new_request(before: &Value, status: &Value) -> Option<(Value, Option<String>, Option<f64>)> {
    let last = &status["last"];
    if !last.is_object() || *last == before["last"] {
        return None;
    }
    let cost = status["cost_usd"]
        .as_f64()
        .map(|total| match before["cost_usd"].as_f64() {
            Some(earlier) if earlier <= total => total - earlier,
            _ => total,
        });
    let model = status["model_id"]
        .as_str()
        .or(status["model"].as_str())
        .map(str::to_string);
    Some((last.clone(), model, cost))
}

/// The folder of the records, in the state folder at `root`.
fn dir(root: &Path) -> PathBuf {
    root.join("usage")
}

/// Adds `record` to its month's file. Appends are one short line each, so
/// conversations writing at once don't mix their lines.
pub fn append(root: &Path, record: &Record) -> Result<()> {
    let dir = dir(root);
    std::fs::create_dir_all(&dir)?;
    let (year, month, _) = civil(record.ts_ms.div_euclid(DAY_MS));
    let mut line = serde_json::to_vec(record)?;
    line.push(b'\n');
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(format!("{year:04}-{month:02}.jsonl")))?
        .write_all(&line)?;
    Ok(())
}

/// Every record from `since_ms` on, and when the first was made.
pub fn read(root: &Path, since_ms: i64) -> (Vec<Record>, Option<i64>) {
    let (since_year, since_month, _) = civil(since_ms.div_euclid(DAY_MS));
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir(root))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == "jsonl"))
        .collect();
    files.sort();
    let mut first = None;
    let mut records = Vec::new();
    for (ix, path) in files.iter().enumerate() {
        let month = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        let wanted = month >= format!("{since_year:04}-{since_month:02}").as_str();
        if !wanted && ix > 0 {
            continue;
        }
        let Ok(file) = std::fs::File::open(path) else {
            continue;
        };
        for record in BufReader::new(file)
            .lines()
            .map_while(Result::ok)
            .filter_map(|line| serde_json::from_str::<Record>(&line).ok())
        {
            if ix == 0 {
                first = Some(first.map_or(record.ts_ms, |f: i64| f.min(record.ts_ms)));
            }
            if wanted && record.ts_ms >= since_ms {
                records.push(record);
            }
        }
    }
    (records, first)
}

const DAY_MS: i64 = 86_400_000;

/// The civil date of `days` since 1970-01-01 (Howard Hinnant's algorithm).
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

fn add(totals: &mut UsageTotals, record: &Record) {
    totals.requests += 1;
    totals.input += record.input;
    totals.cache_read += record.cache_read;
    totals.cache_write += record.cache_write;
    totals.output += record.output;
    totals.reasoning += record.reasoning;
    match record.cost {
        Some(cost) => totals.cost += cost,
        None => totals.unpriced += 1,
    }
}

/// The records from `since_ms` to `now_ms` added up: the period's totals,
/// each day (in the zone `offset_minutes` east of UTC), and the tables by
/// agent and model and by project.
pub fn summarize(
    records: &[Record],
    since_ms: i64,
    now_ms: i64,
    offset_minutes: i32,
) -> UsageStats {
    let offset = i64::from(offset_minutes) * 60_000;
    let day_of = |ms: i64| (ms + offset).div_euclid(DAY_MS);
    let last_day = day_of(now_ms);
    // A day bar for each day of the period, but never more than a year's.
    let first_day = day_of(since_ms).max(last_day - 400);
    let mut days: Vec<UsageDay> = (first_day..=last_day)
        .map(|d| {
            let (y, m, dd) = civil(d);
            UsageDay {
                day: format!("{y:04}-{m:02}-{dd:02}"),
                ..Default::default()
            }
        })
        .collect();
    let mut stats = UsageStats::default();
    let mut models: Vec<UsageRow> = Vec::new();
    let mut projects: Vec<UsageRow> = Vec::new();
    let row = |rows: &mut Vec<UsageRow>, agent: &str, label: &str| -> usize {
        match rows
            .iter()
            .position(|r| r.agent == agent && r.label == label)
        {
            Some(at) => at,
            None => {
                rows.push(UsageRow {
                    agent: agent.into(),
                    label: label.into(),
                    ..Default::default()
                });
                rows.len() - 1
            }
        }
    };
    for record in records
        .iter()
        .filter(|r| r.ts_ms >= since_ms && r.ts_ms <= now_ms)
    {
        add(&mut stats.totals, record);
        if let Some(day) = usize::try_from(day_of(record.ts_ms) - first_day)
            .ok()
            .and_then(|at| days.get_mut(at))
        {
            add(&mut day.totals, record);
            let tokens = record.input
                + record.cache_read
                + record.cache_write
                + record.output
                + record.reasoning;
            match day.agents.iter_mut().find(|(a, _)| *a == record.agent) {
                Some((_, n)) => *n += tokens,
                None => day.agents.push((record.agent.clone(), tokens)),
            }
        }
        let model = record.model.as_deref().unwrap_or("");
        let at = row(&mut models, &record.agent, model);
        add(&mut models[at].totals, record);
        let project = record.project.as_deref().unwrap_or("");
        let at = row(&mut projects, "", project);
        add(&mut projects[at].totals, record);
    }
    for day in &mut days {
        day.agents
            .sort_by_key(|(_, tokens)| std::cmp::Reverse(*tokens));
    }
    models.sort_by_key(|row| std::cmp::Reverse(row.totals.tokens()));
    projects.sort_by_key(|row| std::cmp::Reverse(row.totals.tokens()));
    stats.days = days;
    stats.models = models;
    stats.projects = projects;
    stats
}

/// [`Call::UsageStats`](sandhill_proto::Call::UsageStats) for the state
/// folder at `root`: the last `days` days, today included, in the zone
/// `offset_minutes` east of UTC.
pub fn stats(root: &Path, days: u32, offset_minutes: i32) -> UsageStats {
    let now = crate::journal::now_ms();
    let offset = i64::from(offset_minutes) * 60_000;
    let today = (now + offset).div_euclid(DAY_MS);
    let since = (today - i64::from(days.max(1)) + 1) * DAY_MS - offset;
    let (records, first) = read(root, since);
    let mut stats = summarize(&records, since, now, offset_minutes);
    stats.first_ms = first;
    stats
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(ts_ms: i64, agent: &str, model: &str, project: &str, cost: Option<f64>) -> Record {
        Record {
            ts_ms,
            agent: agent.into(),
            model: Some(model.into()),
            project: Some(project.into()),
            input: 100,
            cache_read: 300,
            output: 50,
            cost,
            ..Default::default()
        }
    }

    #[test]
    fn status_lines_count_each_new_request_once() {
        use serde_json::json;
        let first = json!({"model_id": "claude-opus", "cost_usd": 0.5,
            "last": {"input": 10, "cache_read": 900, "output": 40}});
        let (counts, model, cost) = new_request(&json!(null), &first).unwrap();
        assert_eq!(counts["cache_read"], 900);
        assert_eq!(model.as_deref(), Some("claude-opus"));
        assert_eq!(cost, Some(0.5));
        // The same last request shown again (the status line redraws) is no
        // new request.
        let again = json!({"model_id": "claude-opus", "cost_usd": 0.5,
            "last": {"input": 10, "cache_read": 900, "output": 40}});
        assert_eq!(new_request(&first, &again), None);
        // The next costs what the total rose by.
        let next = json!({"model_id": "claude-opus", "cost_usd": 0.75,
            "last": {"input": 20, "cache_read": 950, "output": 10}});
        assert!((new_request(&first, &next).unwrap().2.unwrap() - 0.25).abs() < 1e-9);
        // A restarted agent counts again from nothing; Codex gives no cost.
        let restarted = json!({"cost_usd": 0.1, "last": {"input": 5}});
        assert_eq!(new_request(&next, &restarted).unwrap().2, Some(0.1));
        let codex = json!({"model": "gpt", "last": {"input": 5, "reasoning": 2}});
        assert_eq!(new_request(&json!({}), &codex).unwrap().2, None);
        // A status with no request yet records nothing.
        assert_eq!(new_request(&json!({}), &json!({"context": 3})), None);
    }

    #[test]
    fn days_are_civil_dates() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(20_773), (2026, 11, 16));
        assert_eq!(civil(-1), (1969, 12, 31));
        assert_eq!(civil(11_016), (2000, 2, 29));
    }

    #[test]
    fn records_add_up_by_day_model_and_project() {
        let day = 20_773 * DAY_MS; // 2026-11-16, 00:00 UTC
        let records = vec![
            record(day + 3_600_000, "claude", "opus", "/p/a", Some(0.5)),
            record(day + 7_200_000, "claude", "opus", "/p/b", Some(0.25)),
            record(day - 3_600_000, "codex", "gpt", "/p/a", None),
            // Before the period.
            record(day - 3 * DAY_MS, "codex", "gpt", "/p/a", None),
        ];
        let stats = summarize(&records, day - DAY_MS, day + DAY_MS / 2, 0);
        assert_eq!(stats.totals.requests, 3);
        assert_eq!(stats.totals.unpriced, 1);
        assert!((stats.totals.cost - 0.75).abs() < 1e-9);
        assert_eq!(stats.totals.tokens(), 3 * 450);
        assert!((stats.totals.cache_rate() - 0.75).abs() < 1e-9);
        assert_eq!(
            stats
                .days
                .iter()
                .map(|d| d.day.as_str())
                .collect::<Vec<_>>(),
            vec!["2026-11-15", "2026-11-16"]
        );
        assert_eq!(stats.days[0].agents, vec![("codex".into(), 450)]);
        assert_eq!(stats.days[1].totals.requests, 2);
        assert_eq!(stats.models[0].label, "opus");
        assert_eq!(stats.models[0].totals.requests, 2);
        // Three hours west, all three fall on the 15th.
        let west = summarize(&records, day - DAY_MS, day + DAY_MS / 2, -180);
        let fifteenth = west.days.iter().find(|d| d.day == "2026-11-15").unwrap();
        assert_eq!(fifteenth.totals.requests, 3);
    }

    #[test]
    fn records_are_kept_by_month_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let day = 20_773 * DAY_MS;
        let october = day - 20 * DAY_MS;
        append(dir.path(), &record(october, "codex", "gpt", "/p", None)).unwrap();
        append(dir.path(), &record(day, "claude", "opus", "/p", Some(1.0))).unwrap();
        let mut names: Vec<String> = std::fs::read_dir(dir.path().join("usage"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, vec!["2026-10.jsonl", "2026-11.jsonl"]);
        let (records, first) = read(dir.path(), day - DAY_MS);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].agent, "claude");
        assert_eq!(first, Some(october));
    }
}
