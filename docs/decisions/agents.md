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
  With "Code File" and no @ file, the agent's code goes into the document
  in front (a new cell below the current one, or the end of a script),
  and only with no document open into the Console (Huan, 2026-11-16;
  `docs/design/done/chat-editor-batch-plan.md`). Before, it ran in the
  Console whenever no file was named.
  A new conversation (Huan, 2026-11-14; `docs/design/done/new-conversation-plan.md`)
  shows the agent centred (mark, name, model and account, the project's live
  sessions as chips), then over the message box "Start with" rows that only
  fill the box (an open file or a live table as @ chips, never sent by
  themselves), "Continue" with the three latest conversations, and the @ / #
  / Enter hints.
  Explore a Table conversations (Huan, 2026-11-19) start with the session's
  tables as visible, removable @ chips; elsewhere messages still carry
  only what the user adds.
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
  account used with Sandhill; the user may turn it on. The page lists the
  turned-on agents first, then the others, each by name; the order is set
  when the page opens (Huan, 2026-11-16). The page has one box
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
  agent's edits is only in Welcome → General. How full the conversation
  is shows as a ring and a percentage, "◔ 42%" (Huan, 2026-11-16).
- "/" in the message box (Huan, 2026-11-16; `docs/design/done/slash-plan.md`):
  typed at the start of a message (or from the @ list's "Commands and
  skills…"), it lists Sandhill's own (Compact while a conversation runs,
  its bioinformatics guides), then the agent's commands and skills. A pick
  is a chip, one per message, first; Compact runs at once. Codex's skills go
  as Codex's skill input; every other pick goes as "/name text"; a guide
  puts its instruction first. Claude Code's and Antigravity's lists come
  from their command and skill folders on the device (names and
  descriptions only), Codex's and pi's from asking them, OpenScience's and
  Oh My Pi's from their running conversation; Posit Assistant keeps its
  own screen's "/".
- Conversation tools (Huan, 2026-11-16; `docs/design/done/chat-editor-batch-plan.md`):
  Up in the message box (caret on its first line) brings back this
  conversation's earlier messages, Down comes forward and past the newest
  returns the unsent text. The user's messages can be selected like
  replies; right-clicking either offers Copy (the selection, else the whole
  message) and Send to Message Box. Every code block in a reply, and the
  code of an agent's steps (coloured like the editor, selectable), has Copy
  and ⋯: Insert at Cursor, Add Cell Below (in the code's language),
  Run in Console, New File (`untitled.py`…). Under each table in a reply:
  Copy (tab-separated), Save as CSV… (on this Mac) and Open in Data
  Explorer, which makes `ai_table_N` in the Console's session with a run
  shown as "run by Sandhill". Agents end a finished reply with one or two
  suggestions in a hidden line (`<!-- next: … | … -->`); the latest
  reply's show as chips, and a click only fills the box. The cell toolbar's
  Ask AI and the editor's right-click "Ask AI About Selection" put the cell
  (a # chip) or the selected code (a chip named "file:12–18") in the box
  and send nothing.
- Context panel (Huan, 2026-11-16; `docs/design/done/context-plan.md`):
  for Claude Code and Codex, the ring in the box opens a panel with how
  full the conversation is (number, %, a bar of the last request's parts,
  room left), the last request (read from cache, written to cache, new
  input, reply, Codex's reasoning), the conversation's totals (Claude Code
  also cost "at API prices", time and lines changed), Codex's instruction
  files, and actions: Compact (Claude Code with an optional "keep in
  mind"), Claude Code's full breakdown (its own /context), Codex's Undo
  last exchange and Continue as new, and "New conversation with a summary"
  (the agent writes a summary, which opens attached to a new conversation
  with the same agent, model and mode). Other agents keep the plain popup.

## The agents
- Paused (Huan, 2026-11-18): only Claude Code and Codex run, until they fit
  Sandhill well. Antigravity, OpenScience, Oh My Pi, pi and Posit
  Assistant are on `sandhill_proto::PAUSED_AGENTS`: the daemon doesn't
  list, start, restart, sign in or ask them anything (not even
  `--version`), and the app offers them nowhere, Welcome → Agents
  included. Their old conversations stay readable. Their code and tests
  are kept; tests that reach them through requests check the refusal
  instead. To bring one back, take it off the list. Why: Google suspended
  an account whose Antigravity Sandhill drove, and its FAQ says wrapping
  `agy` in another app isn't supported on personal plans; Oh My Pi and pi
  are only as safe as the logins they hold.
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
- Skills (Huan, 2026-11-16; `docs/design/done/skills-plan.md`): Welcome → Skills
  installs skills (folders with a `SKILL.md`) per device, from Sandhill's
  Suggested list, GitHub, or a folder or zip on the Mac, after a card shows
  the source, licence, files and any scripts; Copy to… sends one to another
  device. Sandhill keeps them in its own state folder and gives them to
  every agent through its `get_skill` tool and "/", never through the
  agents' own folders. Suggested is only skills published under a
  GPL-compatible licence, fetched at a pinned commit on Add: Anthropic's
  eight Apache-2.0 science skills. Claude Science's own skills and
  connectors are inside its app and are not copied. No Sandhill-written
  skills for other tools until Huan asks. No automatic updates. Connectors
  come after skills, in their own plan.
- Connectors (Huan, 2026-11-16; `docs/design/done/connectors-plan.md`):
  Welcome → Connectors adds MCP servers per device (Sandhill's list, a web
  address with an optional key, or a command), with Copy to…. The daemon
  holds the connections and every agent reaches them through Sandhill's
  tool server (`list_connectors`, `query_connector`, `call_connector`), never
  through the agents' own settings; only connectors from Sandhill's list
  run without the agent's approval. Sandhill's list: BioMCP, BioContextAI
  Knowledgebase and Open Targets, none needing an account. Anthropic's
  hosted PubMed, bioRxiv, Clinical Trials and ChEMBL addresses are left
  out. Sign-in connectors (Huan, 2026-11-16, phase 2; `docs/design/done/
  connector-sign-in-plan.md`): Consensus, Synapse, Wiley Scholar Gateway
  and BioRender join the list; the browser on the Mac signs in to the
  user's own account, the device keeps and renews the tokens, and nothing
  is copied between devices.

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
- Usage (Huan, 2026-11-16; `docs/design/done/usage-stats-plan.md`): a
  Welcome page beside Agents, one device at a time, like `omp stats`. For
  7 days, 30 days, 90 days or a year: requests, tokens (in and out), cache
  rate, estimated cost and active days; a bar for each day, coloured by
  agent; tables by agent and model and by project. Sandhill counts only
  its own conversations, from when this was built: each model request an
  agent reports to it (Claude Code's status line, Codex's token reports,
  pi's replies, OpenScience's and Oh My Pi's turns), kept on the device in
  `usage/<month>.jsonl` of its state folder. It never reads the agents'
  own folders. Costs are the agents' own estimates at list prices; Codex
  gives none, so its cost shows "–". Read when shown and on Refresh, never
  on a timer.
