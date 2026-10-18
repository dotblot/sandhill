//! Sandhill's own modes on top of an agent's: **Restricted** asks the user
//! before every step, and **Ask** is read-only (no edits, no code runs; the
//! agent suggests changes in the chat instead). Neither agent has them as
//! such, so Sandhill keeps them as the terminal's `guard`, in its report
//! (`agent.json`), and enforces them where it can see each step:
//!
//! - Claude Code runs in its default mode, and Sandhill's `PreToolUse`
//!   hook asks the user about each step, or blocks the ones that change
//!   something (see [`claude`]).
//! - Codex gets its strictest settings ([`super::codex`]): Restricted asks
//!   before anything not known to be harmless, and Ask runs in a read-only
//!   sandbox and never asks, so Codex refuses tools that need asking.
//! - Sandhill's MCP server refuses its tools that change something in Ask
//!   mode, whichever agent calls them.
//!
//! The guard is set from the mode an agent starts in, and by `set` commands
//! (`{kind: "set", permission}`) from the app.

use std::path::Path;

use serde_json::Value;

pub const RESTRICTED: &str = "restricted";
pub const ASK: &str = "ask";

/// What the agent is told with each message in Ask mode.
pub const ASK_NOTE: &str = "Sandhill is in Ask mode, which is read-only: don't edit files, run \
    commands or run code. Look with the read-only tools, and suggest changes and code in your \
    reply instead; the user can switch modes to have you carry them out.";

/// The guard a permission mode (`AgentOptions::permission`) sets, if any.
pub fn of(permission: Option<&str>) -> Option<&'static str> {
    match permission {
        Some(RESTRICTED) => Some(RESTRICTED),
        Some(ASK) => Some(ASK),
        _ => None,
    }
}

/// The guard on the terminal in `dir` now. Read from its report each time:
/// the user can switch modes while the agent runs.
pub fn current(dir: &Path) -> Option<String> {
    let report: Value = super::read_json(&super::Paths::new(dir).agent())?;
    report["guard"]
        .as_str()
        .filter(|g| of(Some(g)).is_some())
        .map(str::to_string)
}

/// What the chat says of a step blocked in Ask mode.
pub const BLOCKED_STEP: &str = "Not run: Ask mode is read-only.";

/// Why a step was blocked in Ask mode, for the agent.
pub const BLOCKED: &str = "Sandhill is in Ask mode, which is read-only, so this step was not \
    run. Suggest the change or the code in your reply instead.";

/// What Sandhill does with a step of Claude Code's under a guard.
#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Leave it to Claude Code's own permissions.
    Pass,
    /// Ask the user first.
    Ask,
    /// Block it.
    Block,
}

/// Claude Code's tools that only look, or only talk to the user.
const CLAUDE_READ_ONLY: [&str; 18] = [
    "Read",
    "Glob",
    "Grep",
    "LS",
    "NotebookRead",
    "WebFetch",
    "WebSearch",
    "TodoWrite",
    "TodoRead",
    "Task",
    "Agent",
    "AskUserQuestion",
    "ExitPlanMode",
    "EnterPlanMode",
    "ListMcpResourcesTool",
    "ReadMcpResourceTool",
    "ToolSearch",
    "BashOutput",
];

/// Claude Code's tools that aren't steps on the user's things: its to-do
/// list, its questions and plans (which ask in their own way), and loading
/// tools.
const CLAUDE_NOT_STEPS: [&str; 6] = [
    "TodoWrite",
    "TodoRead",
    "AskUserQuestion",
    "ExitPlanMode",
    "EnterPlanMode",
    "ToolSearch",
];

/// What to do with Claude Code's `tool` under `guard`. Subagents' steps
/// come through the same hook, so they are guarded too.
pub fn claude(guard: &str, tool: &str) -> Verdict {
    match guard {
        RESTRICTED if CLAUDE_NOT_STEPS.contains(&tool) => Verdict::Pass,
        RESTRICTED => Verdict::Ask,
        ASK if read_only(tool) => Verdict::Pass,
        ASK => Verdict::Block,
        _ => Verdict::Pass,
    }
}

/// Whether Claude Code's `tool` only looks: its own read-only tools, and
/// Sandhill's.
fn read_only(tool: &str) -> bool {
    if let Some(name) = tool.strip_prefix("mcp__sandhill__") {
        return crate::mcp::read_only_tools().iter().any(|t| t == name);
    }
    CLAUDE_READ_ONLY.contains(&tool)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restricted_asks_before_every_step_and_ask_only_looks() {
        assert_eq!(claude(RESTRICTED, "Read"), Verdict::Ask);
        assert_eq!(claude(RESTRICTED, "Bash"), Verdict::Ask);
        assert_eq!(
            claude(RESTRICTED, "mcp__sandhill__list_variables"),
            Verdict::Ask
        );
        assert_eq!(claude(RESTRICTED, "AskUserQuestion"), Verdict::Pass);
        assert_eq!(claude(ASK, "Read"), Verdict::Pass);
        assert_eq!(claude(ASK, "mcp__sandhill__list_variables"), Verdict::Pass);
        assert_eq!(claude(ASK, "mcp__sandhill__run_code"), Verdict::Block);
        assert_eq!(claude(ASK, "mcp__sandhill__add_cell"), Verdict::Block);
        assert_eq!(claude(ASK, "Edit"), Verdict::Block);
        assert_eq!(claude(ASK, "Bash"), Verdict::Block);
        // Another server's tools may change anything.
        assert_eq!(claude(ASK, "mcp__github__create_issue"), Verdict::Block);
    }

    #[test]
    fn the_guard_comes_from_the_report() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(current(dir.path()), None);
        std::fs::write(dir.path().join("agent.json"), r#"{"guard": "ask"}"#).unwrap();
        assert_eq!(current(dir.path()).as_deref(), Some(ASK));
        std::fs::write(dir.path().join("agent.json"), r#"{"guard": null}"#).unwrap();
        assert_eq!(current(dir.path()), None);
        std::fs::write(dir.path().join("agent.json"), r#"{"guard": "full"}"#).unwrap();
        assert_eq!(current(dir.path()), None);
        assert_eq!(of(Some("restricted")), Some(RESTRICTED));
        assert_eq!(of(Some("supervised")), None);
    }
}
