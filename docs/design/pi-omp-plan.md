# Plan: pi and Oh My Pi in the AI pane

Status: approved by Huan on 2026-11-12: pi gets chat and command line (A);
if pi cannot take MCP servers per run, add only a `sandhill` entry to
`~/.pi/agent/mcp.json`; use the official marks for both logos.

Implemented 2026-11-12 and checked with real turns on this Mac (omp with
Claude Haiku 4.5, pi with GPT-5.6 Luna; chat, then command line and back).
Decisions while building:
- pi takes Sandhill's MCP server per run through a small extension loaded
  with `-e`, so no `~/.pi` file is edited after all.
- omp lists models with `models --json`; dated snapshots listed beside their
  alias (`claude-haiku-4-5-20251001` beside `claude-haiku-4-5`) are shown
  once, as the alias. Model and effort switch through ACP config options.
- Logos have no background (Huan): Oh My Pi's near-white mark becomes mid
  grey so it reads on light and dark; pi's colour mark is unchanged.
- Streamed reasoning repeats one step id with more text; the chat now
  updates that row instead of adding one per update (this also affected
  OpenScience).
- Not tried on ubuntu-lab, where the server daemon must first be updated.

Two more agents in the AI pane's provider picker, beside Antigravity, Claude
Code, Codex, OpenScience and Posit Assistant:

- **pi** (`pi`, earendil-works/pi, MIT), installed here and on ubuntu-lab as
  1.5.2.
- **Oh My Pi** (`omp`, can1357/oh-my-pi, MIT, a pi fork), installed here as
  18.16.0 and on ubuntu-lab as 18.18.0.

Both use the user's own model accounts and keys, so neither shows quota
(Huan). Both run as unmodified CLIs; Sandhill never reads their credentials.

## Oh My Pi: chat through ACP

`omp acp` speaks the Agent Client Protocol, which Sandhill's generic ACP
bridge (`sandhilld acp-chat`, built for OpenScience) already drives:

- Chat mode: streamed answers, tool steps, approval cards
  (`session/request_permission`), resume through `session/load`.
- Models: `omp models --flat`, read without a model turn (the bridge's
  existing `list_models`). Thinking effort uses omp's `model:<effort>` form.
- Command-line mode: omp's own TUI in the AI pane's terminal, as for Codex
  and Antigravity, switching both ways on the same session.
- Sandhill's tools (Console sessions, notebooks, variables) reach omp through
  the bridge's loopback HTTP MCP server with a per-conversation token, passed
  in the ACP session request, so no omp config file is edited.
- omp's own built-in tools stay available; Sandhill code runs show "run by
  Oh My Pi" in the Console like other agents.

## pi: chat through its RPC mode

pi has no ACP. Its machine interface is `pi --mode rpc` (JSON lines over
stdio: prompts in; message, tool and settle events out), which is stable and
documented. Two ways:

- **A. Chat and command line (recommended):** a small `sandhilld pi-chat`
  bridge translates pi's RPC events into the same chat entries the other
  agents produce, so pi gets the chat view, approvals, model and effort
  pickers (`pi --list-models`, `--thinking`), resume (`--session`), and the
  TUI in command-line mode. About the size of the Antigravity bridge.
- **B. Command line only:** pi always runs its own TUI in the AI pane, like
  Posit Assistant. Much less work, but no chat view, no @/# mentions in
  Sandhill's composer, and models/effort are picked inside pi.

Sandhill's MCP tools reach pi through pi's MCP support. pi reads MCP servers
from `~/.pi/agent/mcp.json` or the project's `.pi/mcp.json`; Sandhill would
pass its server without editing either file if pi accepts it per run (to be
confirmed); otherwise it adds only a `sandhill` entry to `~/.pi/agent/mcp.json`
and leaves everything else in that file untouched, as it does for Claude Code.

## Icons, quota, install

- Logos: Huan supplies them (or the official marks: pi.dev's logo, Oh My Pi's
  repository mark), vendored under `docs/design/ui/icons/vendor/brands/` with
  backgrounds removed, the mark only, with their licences recorded.
  `agent.omp` replaces today's placeholder; `agent.pi` is new.
- Quota: neither appears in the status bar quota chips or popup.
- Not installed on a device: the picker shows the agent with an install note,
  like the others; Sandhill does not install either CLI.

## Verification

Real `omp` and `pi` on this Mac and ubuntu-lab: a chat turn, an approval, a
Sandhill tool call into a live Python session, model and effort switching,
resume after reconnect, and chat/command-line switching. Integration tests
use the real CLIs when present and skip with a message otherwise.
