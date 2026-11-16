//! What the agents in Sandhill's conversations used on a device, as
//! [`crate::Call::UsageStats`] adds it up (Huan, 2026-11-16).

use serde::{Deserialize, Serialize};

/// Requests, tokens and cost of some of the recorded replies.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct UsageTotals {
    /// Model requests (a turn, for agents that report only turns).
    pub requests: u64,
    /// New input tokens, not read from or written to the cache.
    pub input: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub output: u64,
    /// Reasoning tokens, where the agent counts them apart from output.
    pub reasoning: u64,
    /// The agents' own cost estimates (list prices), in US dollars.
    pub cost: f64,
    /// Requests whose agent gave no cost.
    pub unpriced: u64,
}

impl UsageTotals {
    pub fn tokens(&self) -> u64 {
        self.input + self.cache_read + self.cache_write + self.output + self.reasoning
    }

    /// The share of input read from the cache, 0..=1.
    pub fn cache_rate(&self) -> f64 {
        let read = self.cache_read as f64;
        let all = read + self.input as f64 + self.cache_write as f64;
        if all > 0.0 { read / all } else { 0.0 }
    }
}

/// One day, in the asker's time zone.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct UsageDay {
    /// `2026-11-16`.
    pub day: String,
    /// Tokens by agent kind, largest first.
    pub agents: Vec<(String, u64)>,
    pub totals: UsageTotals,
}

/// A row of a table: an agent and a model, or a project.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct UsageRow {
    /// The agent's kind (`claude`, `codex`…); empty for a project row.
    pub agent: String,
    /// The model, or the project's folder.
    pub label: String,
    pub totals: UsageTotals,
}

/// The answer to [`crate::Call::UsageStats`].
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct UsageStats {
    pub totals: UsageTotals,
    /// Every day of the period, oldest first, empty ones too.
    pub days: Vec<UsageDay>,
    /// By agent and model, most tokens first.
    pub models: Vec<UsageRow>,
    /// By project, most tokens first.
    pub projects: Vec<UsageRow>,
    /// When the device's first record was made, if any: before that,
    /// nothing was recorded.
    pub first_ms: Option<i64>,
}
