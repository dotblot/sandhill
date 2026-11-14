//! Antigravity's official read-only `/usage` command, separate from chat turns.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};
use tokio::io::{AsyncRead, AsyncReadExt};

const QUERY_TIMEOUT: Duration = Duration::from_secs(30);
const OUTPUT_LIMIT: u64 = 1024 * 1024;

#[derive(Debug)]
pub(crate) struct Usage {
    pub(crate) text: String,
    /// A full provider snapshot, not an account identity or an incremental feed.
    pub(crate) report: Value,
}

pub(crate) async fn read_usage(agy: &Path, cwd: Option<&Path>) -> Result<Usage> {
    query(agy, cwd, QUERY_TIMEOUT, OUTPUT_LIMIT).await
}

async fn query(agy: &Path, cwd: Option<&Path>, deadline: Duration, limit: u64) -> Result<Usage> {
    let mut command = super::account_command(agy);
    command
        .args(["--print", "/usage", "--output-format", "json"])
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    super::headless(&mut command);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    if let Some(path) = crate::agents::path_for(agy)? {
        command.env("PATH", path);
    }
    let mut child = command.spawn().context("cannot start Antigravity /usage")?;
    let pid = child.id();
    let stdout = child
        .stdout
        .take()
        .context("Antigravity /usage has no stdout")?;
    let stderr = child
        .stderr
        .take()
        .context("Antigravity /usage has no stderr")?;
    let result = tokio::time::timeout(deadline, async {
        tokio::try_join!(bounded(stdout, limit), bounded(stderr, limit), async {
            child.wait().await.map_err(anyhow::Error::from)
        })
    })
    .await;
    // A CLI child or a descendant holding a pipe must not survive a failed query.
    if !matches!(&result, Ok(Ok(_))) {
        if let Some(pid) = pid {
            super::signal_group(pid, libc::SIGKILL);
        }
        let _ = child.start_kill();
        let _ = tokio::time::timeout(Duration::from_secs(1), child.wait()).await;
    }
    let (stdout, stderr, status) =
        result.context("Antigravity /usage took longer than its deadline")??;
    if !status.success() {
        bail!(
            "Antigravity /usage exited with {status}: {}",
            String::from_utf8_lossy(&stderr).trim()
        );
    }
    let value: Value =
        serde_json::from_slice(&stdout).context("Antigravity /usage did not return valid JSON")?;
    parse(&value, crate::journal::now_ms())
}

async fn bounded(reader: impl AsyncRead + Unpin, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(limit + 1).read_to_end(&mut bytes).await?;
    if bytes.len() as u64 > limit {
        bail!("Antigravity /usage output exceeded {limit} bytes");
    }
    Ok(bytes)
}

fn parse(value: &Value, observed: i64) -> Result<Usage> {
    if value["command"]["name"] != "usage" {
        bail!("Antigravity did not return its official usage command result");
    }
    if value["num_turns"].as_u64().is_some_and(|turns| turns != 0)
        || value["conversation_id"]
            .as_str()
            .is_some_and(|id| !id.is_empty())
    {
        bail!("Antigravity /usage unexpectedly returned a conversation");
    }
    if value["status"] != "SUCCESS" {
        bail!("Antigravity /usage did not report success");
    }
    let groups = value["command"]["data"]["groups"]
        .as_array()
        .context("Antigravity /usage did not report quota groups")?;
    let text = value["response"]
        .as_str()
        .context("Antigravity /usage did not report display text")?
        .to_owned();
    let mut windows = Map::new();
    for group in groups {
        let name = group["name"]
            .as_str()
            .filter(|name| !name.is_empty())
            .context("Antigravity /usage reported an unnamed quota group")?;
        let buckets = group["buckets"]
            .as_array()
            .context("Antigravity /usage reported invalid quota buckets")?;
        for bucket in buckets {
            let id = bucket["id"]
                .as_str()
                .filter(|id| !id.is_empty())
                .context("Antigravity /usage reported a quota bucket without an id")?;
            let window = bucket["window"]
                .as_str()
                .filter(|window| !window.is_empty())
                .context("Antigravity /usage reported a quota bucket without a window")?;
            let key = serde_json::to_string(&[name, window, id])?;
            let remaining = bucket["remaining_fraction"]
                .as_f64()
                .filter(|remaining| remaining.is_finite() && (0.0..=1.0).contains(remaining));
            let mut quota = json!({"label": format!("{name} · {window}"),
                "used_percentage": remaining.map(|remaining| 100.0 * (1.0 - remaining)),
                "observed_at_ms": observed});
            if let Some(reset) = bucket.get("reset_time") {
                quota["resets_at"] = reset.clone();
            }
            if windows.insert(key, quota).is_some() {
                bail!("Antigravity /usage reported a duplicate quota bucket");
            }
        }
    }
    Ok(Usage {
        text,
        report: json!({"kind": "antigravity", "quota_source": "cli_snapshot",
            "observed_at_ms": observed, "status": {"quota_observed_at_ms": observed,
                "rate_limits": windows}}),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn official_usage() -> Value {
        json!({"status": "SUCCESS", "num_turns": 0, "conversation_id": "",
        "response": "Gemini Models\tweekly\t99.37% remaining\nClaude and GPT models\t5h\t100% remaining",
        "command": {"name": "usage", "data": {"groups": [
            {"name": "Gemini Models", "description": "Models within Gemini",
                "buckets": [
                    {"id": "gemini-weekly", "window": "weekly", "remaining_fraction": 0.9937466382980347, "reset_time": "2026-11-17T18:03:02Z"},
                    {"id": "gemini-5h", "window": "5h", "remaining_fraction": 0.9899333119392395, "reset_time": "2026-11-10T23:03:02Z"}]},
            {"name": "Claude and GPT models", "buckets": [
                {"id": "3p-weekly", "window": "weekly", "remaining_fraction": 1, "reset_time": "2026-11-17T18:11:36Z"},
                {"id": "3p-5h", "window": "5h", "remaining_fraction": 1, "reset_time": "2026-11-10T23:11:36Z"}]}
        ]}}})
    }

    #[test]
    fn official_groups_are_distinct_and_remaining_is_converted_to_used() {
        let raw = official_usage();
        let usage = parse(&raw, 1000).unwrap();
        assert_eq!(usage.text, raw["response"].as_str().unwrap());
        assert!(usage.report.get("account").is_none());
        let windows = usage.report["status"]["rate_limits"].as_object().unwrap();
        assert_eq!(windows.len(), 4);
        let gemini = &windows
            [&serde_json::to_string(&["Gemini Models", "weekly", "gemini-weekly"]).unwrap()];
        assert_eq!(gemini["label"], "Gemini Models · weekly");
        assert!(
            (gemini["used_percentage"].as_f64().unwrap() - 0.6253361701965332).abs() < 0.000001
        );
        let third_party =
            &windows[&serde_json::to_string(&["Claude and GPT models", "5h", "3p-5h"]).unwrap()];
        assert_eq!(third_party["used_percentage"], 0.0);
    }

    #[test]
    fn malformed_remaining_is_unknown_and_missing_reset_is_not_invented() {
        for remaining in [Value::Null, json!(-0.1), json!(1.1), json!("unknown")] {
            let mut raw = official_usage();
            raw["command"]["data"]["groups"][0]["buckets"][0]["remaining_fraction"] = remaining;
            raw["command"]["data"]["groups"][0]["buckets"][0]
                .as_object_mut()
                .unwrap()
                .remove("reset_time");
            let usage = parse(&raw, 1000).unwrap();
            let key = serde_json::to_string(&["Gemini Models", "weekly", "gemini-weekly"]).unwrap();
            let window = &usage.report["status"]["rate_limits"][key];
            assert!(window["used_percentage"].is_null());
            assert!(window.get("resets_at").is_none());
        }
        let mut raw = official_usage();
        raw["command"]["data"]["groups"] = json!(null);
        assert!(parse(&raw, 1000).is_err());
        let mut conversation = official_usage();
        conversation["num_turns"] = json!(1);
        assert!(parse(&conversation, 1000).is_err());
    }

    #[tokio::test]
    async fn subprocess_bounds_output_deadline_and_reports_errors() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let agy = dir.path().join("agy");
        let raw = official_usage().to_string();
        std::fs::write(&agy, format!("#!/bin/sh\ncat <<'USAGE'\n{raw}\nUSAGE\n")).unwrap();
        std::fs::set_permissions(&agy, std::fs::Permissions::from_mode(0o700)).unwrap();
        let usage = read_usage(&agy, Some(dir.path())).await.unwrap();
        assert_eq!(
            usage.report["status"]["rate_limits"]
                .as_object()
                .unwrap()
                .len(),
            4
        );
        assert!(
            query(&agy, Some(dir.path()), QUERY_TIMEOUT, 32)
                .await
                .unwrap_err()
                .to_string()
                .contains("exceeded")
        );
        std::fs::write(&agy, "#!/bin/sh\nexec sleep 30\n").unwrap();
        assert!(
            query(&agy, None, Duration::from_millis(20), OUTPUT_LIMIT)
                .await
                .unwrap_err()
                .to_string()
                .contains("deadline")
        );
        std::fs::write(
            &agy,
            "#!/bin/sh\nprintf 'usage temporarily unavailable' >&2\nexit 7\n",
        )
        .unwrap();
        assert!(
            read_usage(&agy, None)
                .await
                .unwrap_err()
                .to_string()
                .contains("usage temporarily unavailable")
        );
    }
}
