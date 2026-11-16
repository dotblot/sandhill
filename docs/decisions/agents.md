# AI agents

Settled decisions about the AI pane, its agents, accounts and pickers.
Moved here unchanged from `CLAUDE.md` on 2026-11-15; where an older plan
differs, this wins.

## Conversations
- AI conversations belong to the whole project, not to one file. `@file`
  attaches a file to that one message only; `#` picks a notebook cell
  (and brings its output). No @ symbol is shown in the composer box.
  Messages carry no implicit document context (open file, cell or
  selection); only what the user adds with @ and # (Huan, 2026-11-12).
  A new conversation (Huan, 2026-11-14; `docs/design/done/new-conversation-plan.md`)
  shows the agent centred (mark, name, model and account, the project's live
  sessions as chips), then over the message box "Start with" rows that only
  fill the box (an open file or a live table as @ chips, never sent by
  themselves), "Continue" with the three latest conversations, and the @ / #
  / Enter hints.
- New conversation has no separate agent buttons. Agent/model selection stays
  in the composer picker, including Posit Assistant; PA manages models itself.

## Model and effort pickers
- Thinking effort is one segmented bar (Huan, 2026-11-12,
  after a Dribbble reference): the fill reaches the chosen level and carries
  its name, ticks divide the rest, clicks and drags snap, the fill springs and
  glows, and the strongest level is warm. No explanatory paragraph.
  Model popup: fixed 380×360px, icon-only provider tabs on a 32px left rail in
  alphabetical order with Favorites first, names in hover tooltips; the right
  model list scrolls. New conversation starts with Codex (Huan, 2026-11-14).
  Agents are turned on or off on Welcome → Agents (Huan, 2026-11-15), a
  choice of this Mac for every device: off, an agent is not offered, never
  asked for models, accounts or quota, and its old chats take no messages.
  Antigravity starts off, with an amber line saying Google suspended an
  account used with Sandhill; the user may turn it on. The page has one box
  per agent (mark, name, version and plan or "Not installed on …", switch)
  and, for Claude Code, Codex and Antigravity, their accounts: "Default for
  new chats" tag, Sign In when needed, a ⋯ menu (Use for New Chats,
  Rename…, Remove…) and "+ Add Account…". Removing asks in the row; the
  agent's own login is never renamed or removed. Returned effort-variant siblings
  share one row and slider levels map to real IDs, without a second `--effort`.
  Preserve explicit provider/model selections and existing chats on refresh.
- Message box (Huan, 2026-11-16; `docs/design/done/composer-plan.md`):
  chips, then the text across the full width, then one row of pills:
  + (opens the @ list), the mode, "Code File/Console", the context ring,
  the model and the send/stop button. Modes are each agent's own, in its
  own words, starting in its own default (Claude Code: Manual, Accept
  edits, Plan mode, Auto mode, Bypass permissions; Codex: Read Only,
  Default, Approve for me, Full Access, Plan mode; Antigravity: Default,
  Accept edits, Auto, Plan, Skip permissions; OpenScience: Build, Plan;
  Oh My Pi: Always ask, Write, YOLO, Plan). pi has no modes and runs
  everything; Sandhill's own Restricted and Ask modes are gone. There is
  no web-search switch: each agent keeps its own default. Following the
  agent's edits is only in Welcome → General.

## The agents
- Agents: Claude Code and Codex run as unmodified CLIs. Do not read or reuse
  their OAuth tokens. OpenScience runs unmodified over ACP, chat only
  (Huan, 2026-11-11): deny its built-in `python` and `r` tools and direct
  analysis code to Sandhill's live kernels through MCP (served over loopback
  HTTP with a token, as its sandbox blocks the daemon socket). Oh My Pi (`omp`)
  runs unmodified over the same ACP bridge, and pi through its `--mode rpc`
  bridge (`pi-chat`), both with chat and command line, their own model
  accounts and no quota (Huan, 2026-11-12). Pass Sandhill's tools per run
  (ACP session request, per-run extensions/overlays); never edit `~/.omp`,
  and touch `~/.pi` only to add a `sandhill` entry to `agent/mcp.json` if pi
  cannot take MCP per run.

## Accounts and quota
- Agent accounts (Huan, 2026-11-13; `docs/design/done/agent-accounts-plan.md`):
  Claude Code, Codex and Antigravity can each have several accounts per
  device, switched by hand inside a conversation, which resumes with its
  context. Claude/Codex accounts are folders of Sandhill's own
  (`CLAUDE_CONFIG_DIR`/`CODEX_HOME`) linking back to the user's folder for
  everything but the login, signed in once by the agent itself; Antigravity's
  added accounts are Gemini API keys, its own Google login changes only for
  the whole device. The switch lives in the status bar quota panel; at a limit
  the chat offers "Continue with <account>"; nothing switches by itself.
  With no switchable conversation shown, the panel's "Use for new chats" sets
  the account new conversations start with; only it does, switching a
  conversation leaves that default alone. Claude's folder-trust question
  (asked once per account and folder) is pinned above the chat composer.
  Sandhill never reads or rewrites an agent's login. The quota panel (Huan,
  2026-11-13) shows each account as a card: bars and rings show what is
  LEFT (green ≥30%, amber 10–30%, red <10%) with the time to reset; Codex
  cards show banked resets and "Use Now…" (the soonest to expire, confirmed
  inline, through Codex's own app-server); Claude Code cannot report its
  limit resets, so its cards link to claude.ai Settings → Usage.
  Only Claude Code, Codex and Antigravity show quota in the status bar;
  Posit Assistant, OpenScience, Oh My Pi and pi have none (Huan, 2026-11-14).
  Sandhill never asks an agent for its quota or sign-in on its own (Huan,
  2026-11-14, after Google suspended an Antigravity account that Sandhill
  had been checking every minute): only a Refresh click (that provider's
  accounts, or every account on Welcome) or a finished sign-in (that
  account) asks. Otherwise the status bar shows what the last check saved
  on the device, until its window resets; the panel says beside each Refresh
  button how old the numbers are ("Updated 3h ago").
