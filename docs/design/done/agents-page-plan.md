# Welcome's Agents page

Huan, 2026-11-15: "redesign the agent accounts settings in welcome window.
allow user to enable/disable agents, add/remove accounts." Status: approved
and built the same day. Answers: on/off is this Mac's, for every device;
Antigravity starts off (with its amber line) and the user may turn it on;
the row is renamed Agents. Huan also asked for one device name everywhere:
the status bar said "Local" where Welcome says "This Mac".

## What is there now (`agents/accounts.rs`, Manage → Agent Accounts)

- Only Claude Code and Codex are listed (Antigravity is hidden). Oh My
  Pi, pi, OpenScience and Posit Assistant don't appear at all.
- Turning an agent off exists only in the code: a fixed list
  (`TURNED_OFF`, Antigravity since Google suspended the account). The
  user can't turn any agent on or off.
- Accounts are rows of bold name, grey status and a row of plain
  buttons (Sign In, Rename, Remove…), under a bold agent name and "Add
  Account…". It doesn't use the groups, rows, switches and capsule tabs
  that General and Project settings now share, and its text is 13px
  while theirs is 15px.
- "Use for new chats" exists only in the status bar's quota panel.

## The new page

Manage → **Agents** (renamed from Agent Accounts, since it now does
more than accounts), drawn with General's groups and rows:

- **Header:** "Agents", one grey sentence ("Choose which agents Sandhill
  offers, and the accounts they run on. Switching accounts in a
  conversation is always your choice."), Refresh at the right. Under it,
  capsule tabs for the connected devices (This Mac | lab), because
  accounts belong to a device.
- **One group per agent,** in the model menu's order (alphabetical). Its
  first row: the agent's mark, its name, and a grey line: what it runs
  and its version ("Codex 0.62 · your ChatGPT plan"), or "Not installed
  on lab" with its install command to copy. A **switch** on the right
  turns the agent on or off.
- **Turned off,** an agent isn't offered for new conversations and is
  never asked for models, accounts or limits (as `TURNED_OFF` does now);
  its past conversations stay readable. The group shows only its first
  row.
- **Accounts** (Claude Code, Codex, Antigravity), one row each: the
  name, "Default for new chats" as a small tag on the one new chats
  start with, and a grey line (signed in · email · last known use, or
  "Sign in again" in amber). On the right: Sign In when needed, and a
  ⋯ menu with Use for New Chats, Rename…, Remove…. The agent's own
  login (the "Default" account) can't be renamed or removed; Sandhill
  never touches it.
- **Removing** asks in the row, as the interface rules say: the row
  reads "Remove Work?" with Cancel and a red Remove.
- **Adding:** the group's last row is "+ Add Account…", which opens the
  existing name (and, for Antigravity, API key) form.
- **Agents with their own model accounts** (Oh My Pi, pi, OpenScience,
  Posit Assistant) show one grey line instead: "Uses its own model
  accounts; set them up in Oh My Pi."

What adding, signing in and removing do is unchanged; only how the page
looks and where the switches are.

Built differently, after seeing it on screen: each agent's box has no
heading above it, since its first row already names it; the account
rows start under the agent's name. Agents with their own model accounts
keep their plan in the first row's grey line ("Oh My Pi · your own
model accounts") rather than a second line.

## Questions for Huan

1. Turning an agent on or off: kept on this Mac and applied on every
   device (like General's choices), or chosen per device? I suggest
   this Mac: an agent missing on one device already shows "Not
   installed" there.
2. Antigravity stays off, as now, with one amber line in its group:
   "Google suspended an account used with Sandhill; turning it on is
   your choice." Turning it on is then allowed. Agreed?
3. Rename the sidebar row "Agent Accounts" to "Agents". Agreed?
