# Context for Claude Code and Codex

Huan, 2026-11-16: "For Claude Code and Codex, show as much information as
they provide as well as what the user can do with the context. For other
agents, leave them as it is for now."
Status: approved and built 2026-11-16 with Huan's answers: show Claude Code's cost
("at API prices"); include Codex's Undo last exchange and Continue as new;
"New conversation" becomes "New conversation with a summary" (below). The
layout is the mockup shown that day: fullness, last request, this
conversation, Codex's instruction files, then actions.

**New conversation with a summary.** Sandhill sends the agent one more
message in this conversation asking for a summary to continue from (goal,
decisions, current state, files, next steps). When the reply arrives, it
is saved in `.sandhill/attachments/` and a new conversation opens with the
same agent, model and mode, the summary attached as a chip; the user writes
the first message. The old conversation stays in the history.

Today the box shows a ring and "42K"; its popup shows "42K of 200K used"
and a Compact button. Sandhill keeps only the percentage and the window
size of what the agents send.

## What they provide

Checked against Codex's own protocol description (0.169) and Claude Code's
status-line documentation; Claude Code's fields are to be confirmed against
the installed 2.1.325 while building.

| | Claude Code (its status line, after each reply) | Codex (`thread/tokenUsage/updated`, thread start) |
|---|---|---|
| Window size, % used | yes | yes |
| Last request: new input, read from cache, written to cache, output | yes | yes, plus reasoning |
| Whole conversation: input and output totals | yes | yes |
| Cost so far (API prices; on a plan, what it would have cost) | yes | no |
| Time spent, lines added and removed | yes | no |
| Instruction files it loaded (AGENTS.md…) | no | yes |
| Full breakdown (system, tools, memory files, messages, free space) | only on its own screen (`/context`) | no |

## What the user can do

| | Claude Code | Codex |
|---|---|---|
| Compact | `/compact`, with an optional "keep in mind…" focus | `thread/compact/start` |
| See the full breakdown | `/context`, shown in its terminal | — |
| Undo the last exchange (frees its context) | only interactively (`/rewind`) | `thread/revert` |
| Branch into a new conversation from here | — | `thread/fork` |
| Start fresh | a new conversation | a new conversation |

## The design

The ring stays in the box. Its popup becomes a small panel:

- **Top:** "84K of 200K · 42%" and a bar, amber from 70%, red from 90%.
- **Last request:** new input, from cache, written to cache, output (and
  reasoning for Codex), each a number.
- **This conversation:** total input and output; for Claude Code, cost,
  time and lines changed.
- **Codex's instruction files:** their names; project files open on click.
- **Actions:** Compact (Claude Code: with an optional text field for what to
  keep), "Full breakdown" (Claude Code: switches to its terminal and runs
  `/context`), "Undo last exchange" (Codex), "Branch from here" (Codex), and
  "New conversation".

Other agents keep what they have.

## Questions for Huan

1. Show Claude Code's cost? On a Claude plan it is what the conversation
   would have cost at API prices, not a bill.
2. Codex's "Undo last exchange" and "Branch from here": include them? Each
   also changes what the chat shows (the undone exchange goes; the branch
   opens as a new conversation).
