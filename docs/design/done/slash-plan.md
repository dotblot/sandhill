# "/" in the message box: commands and skills

Huan, 2026-11-16: wants "/" and skills in the AI message box.
Status: approved and built 2026-11-16. Huan's answers: picks are chips;
reading Claude Code's and Antigravity's folders is fine; the guides are in
the list.

## What each agent can tell Sandhill

Checked on this Mac (agents' own help, protocol schemas and docs), not yet
by running a conversation.

| Agent | Can Sandhill ask for the list? | How a pick is sent |
|---|---|---|
| Claude Code | No: it runs on its own screen, and nothing reports its commands. Sandhill would read its command and skill folders (the project's `.claude/`, the user's) and add its built-in commands | typed as `/name …`, as now; Claude Code runs it |
| Codex | Skills yes (`skills/list`); its "/" commands exist only on its own screen | the message carries the skill (`{type: "skill", name, path}`) |
| OpenScience, Oh My Pi | Yes: they send their commands as the conversation starts (ACP's `available_commands_update`) | as the message text `/name …` |
| pi | Yes (`get_commands`: extension commands, prompt templates, skills) | as the message text `/name …`; pi expands it |
| Antigravity | No list found. Its headless mode expands `/skill` text, and skills live in `.agents/skills/` | as the message text, after checking where else its skills live |
| Posit Assistant | Its own screen already has "/" | unchanged |

Sandhill's own: **Compact** (now only in the context popup) and its four
bioinformatics guides (study design, single-cell with Scanpy or Seurat,
results review), which agents already read through Sandhill's tools.

## The design

- Typing `/` at the start of the message opens a list above the box, like
  the `@` list: **Sandhill** (Compact, the guides), **Commands**,
  **Skills**, each row with its name and a short description. Typing narrows
  it; ↑↓, Enter or Tab pick; Esc closes.
- A pick becomes a chip at the start of the message (like `@` chips, with
  a / or skill icon and ×); what you type after it is its text. Compact
  runs at once, as the popup's button does.
- A guide's chip tells the agent to follow that Sandhill guide for this
  message.
- The `+` list gets a row "Commands and skills…" that opens the same list.
- A new conversation shows the agent's list before the first message where
  the agent can say it without starting a conversation (Codex, pi; the
  folders for Claude Code and Antigravity). OpenScience and Oh My Pi tell
  theirs only once a conversation runs; before that the list shows
  Sandhill's own and says theirs come with the first message.

## Not changed

`@` and `#`; typing `/…` by hand still goes to the agent as now.

## Questions for Huan

1. Picks as chips (above), or as plain text `/name` in the box?
2. Claude Code and Antigravity can't be asked: is reading their skill and
   command folders on the device (names and descriptions only) fine?
3. Sandhill's guides in the list: yes?
