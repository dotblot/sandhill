# Splitting the daemon's `server.rs`

Plan (Huan asked for it on 2026-11-14 and approved it the same day; built,
see "As built" at the end). It follows
`docs/design/done/file-split-plan.md`, which left this file out because it cannot
be split by moving code alone.

## What is there
`crates/daemon/src/server.rs` is 2,277 lines. Half of it is one function,
`Daemon::call` (lines 676 to 1822): a single `match` with an arm for each of
the protocol's 107 calls. 65 arms are short, most of them one line that
hands the call to code elsewhere. The other 42 do their work in place and
hold 880 of the function's 1,150 lines: `TableRows` is 60 lines, `TableExport`
38, `TableSummary` 37, `RedrawPlot` 35, `StartSession` 33, and so on.

## What changes
1. **Each long arm's body becomes a method of `Daemon`**, named after its
   call (`Call::TableRows { .. } => self.table_rows(..).await?`). The body
   moves as it is; what the arm bound from the call becomes the method's
   parameters.
2. **The methods go into child modules by subject**, as the protocol crate
   is now divided:

   | New file | Calls |
   |---|---|
   | `server/sessions.rs` | starting sessions, IRkernel, plots and their redrawing, previews, pages |
   | `server/tables.rs` | table rows, summaries and export; variables |
   | `server/help.rs` | help topics, pages and source; packages and updates |
   | `server/documents.rs` | notebooks and files read and written; adding, editing and deleting cells; cell suggestions |
   | `server/terminals.rs` | terminals, jobs, agents' accounts |
   | `server/system.rs` | ping, system statistics, logs, project settings, blobs |

3. **`call` keeps its one `match`**, now a list of about 107 short arms
   (roughly 330 lines). `server.rs` comes to about 1,400 lines, and no file
   made from it passes 400.
4. The helpers at the end of `server.rs` (`quietly`, `redraw_plot`, `live`,
   `session_info`, `forward_events`…) and the connection handling stay.

## What does not change
- **The wire protocol**: no call, parameter, result or error text changes.
- **Behaviour**: each body runs the same statements in the same order.
- **The single `match`**: adding a call to the protocol without handling it
  here stays a compile error. Dividing `call` into one function per subject,
  each with its own `match`, would lose that (a forgotten call would only
  fail when used), so it is not done.
- The order of locks and of `check_trust`, which `handle` does before `call`.

## Why this is more than moving
A body lifted into a method can differ from the arm it was in three ways,
each of which the compiler or a test catches, but which make this a change
to code and not a move:
- `?` and `return` now leave the method, not `call`. Every arm ends in `?`
  or a value today, so this is the same; arms using an early `return
  Ok(...)` are checked one by one.
- Values the arm borrowed from `self` across an `.await` must still be
  allowed to; the compiler rejects what is not.
- An arm that shares a binding with the arm before it (the two
  `RedrawPlot` and `RestartTerminal` arms are guarded pairs) becomes one
  method taking the guard's condition.

## How it is checked
- `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings` on
  the Mac, and clippy for Linux through `cargo zigbuild` (code here differs
  by platform, and the Mac's clippy missed that once).
- `cargo test --workspace` twice. Every call has an integration test with a
  real kernel, which is what makes this safe: 51 groups pass today.
- A line count: every statement of the old function is in exactly one place
  afterwards.
- One commit, with nothing else in it.

## Not in this change
`handle` (120 lines), the connection loop and the tests in `server.rs`.

## As built (2026-11-14)
- 36 bodies moved (every arm over 8 lines, and the cell suggestion), into
  `server/session_calls.rs`, `table_calls.rs`, `help_calls.rs`,
  `document_calls.rs`, `terminal_calls.rs` and `system_calls.rs`, 108 to 198
  lines each. They are named `…_calls` because `server.rs` already uses the
  crate's `tables` and `help` modules, which child modules of those names
  would hide.
- `server.rs` is 1,678 lines and `call` about 540, not the 1,400 and 330
  estimated: a call with several parameters still takes a line for each in
  the `match`, twice (once to take them from the call, once to pass them
  on). The work itself is out of the function; what is left is the table.
- The guarded pairs stayed two arms each: `redraw_plot_shown` and
  `redraw_plot_saved`, `restart_terminal_job` and `restart_terminal_agent`.
- The only changes to the bodies: the last expression is wrapped in `Ok(…)`,
  and rustfmt dropped braces around two one-expression closures.
