//! Idle retirement is requested by the daemon, but checked in the host that
//! observes activity even while no client is connected.

use std::time::{Duration, Instant};

use sandhill_proto::{TERMINAL_JOB, TERMINAL_SHELL};
use serde_json::Value;

const LIMIT: Duration = Duration::from_secs(30 * 60);
const CHECK_INTERVAL: Duration = Duration::from_secs(60);

/// Internal test override, read once when the daemon starts serving.
/// Invalid or zero values leave the normal policy unchanged.
pub(crate) fn timing() -> (Duration, Duration) {
    let limit = std::env::var("SANDHILL_TEST_AGENT_IDLE_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|ms| *ms > 0)
        .map(Duration::from_millis)
        .unwrap_or(LIMIT);
    (
        limit,
        CHECK_INTERVAL.min(limit / 4).max(Duration::from_millis(1)),
    )
}

pub(super) fn is_agent(kind: &str) -> bool {
    kind != TERMINAL_SHELL && kind != TERMINAL_JOB
}

pub(super) fn resumable_idle(kind: &str, report: &Value) -> bool {
    is_agent(kind)
        && kind != "posit"
        && report["state"] == "idle"
        && report[if kind == "claude" {
            "session_id"
        } else {
            "thread"
        }]
        .as_str()
        .is_some_and(|id| !id.is_empty())
}

pub(super) struct Activity {
    last: Instant,
    pub stopped: bool,
}

impl Activity {
    pub fn new() -> Self {
        Self {
            last: Instant::now(),
            stopped: false,
        }
    }

    pub fn touch(&mut self) {
        self.last = Instant::now();
    }

    pub fn expired(&self, limit: Duration) -> bool {
        !self.stopped && self.last.elapsed() >= limit
    }
}
