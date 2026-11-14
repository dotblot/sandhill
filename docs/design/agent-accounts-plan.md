# Plan: several accounts per agent, switched inside a conversation

Status: approved by Huan on 2026-11-13 with every recommendation, except
where the switch is: in the panel the status bar's quota chip opens (Huan),
not a chip in the chat header.

Implemented 2026-11-13 (`crates/daemon/src/accounts.rs`,
`gpui/src/agent_accounts.rs`, `quota.rs`, Welcome → Manage → Agent
Accounts). Checked in the app: the Claude and Codex panels list accounts with
identity and use; Add Account opens Claude Code's own first-run screen in the
sign-in dialog; a Codex conversation switched to a second (unsigned) account
resumed the same thread through the linked `sessions`, switched back from the
panel, and still knew a word from before both switches; the chat shows
"Switched to …" dividers; an Antigravity key is added to and removed from
Sandhill's own Keychain item; removing accounts left `~/.claude` and
`~/.codex` intact. Not checked: a real second login (it needs Huan's
accounts) and the limit banner at a real limit. Finished sign-in terminals
are cleared when the next sign-in starts.

Banked resets (Huan, 2026-11-13): each Codex account's row in the quota panel
shows how many banked resets it has and when the first expires, from Codex's
official `account/rateLimits/read` (`rateLimitResetCredits`). Claude Code has
no official way to read its limit resets (Anthropic: they are only in
claude.ai Settings → Usage), so Claude rows link there instead; Sandhill does
not read Claude Code's internal caches or call claude.ai with its login.

Huan: with several Claude, Codex or Antigravity accounts, switch a
conversation to another account by hand when one runs out of quota, and
keep working in the same conversation with its context. Needed often.

## What each agent allows (checked 2026-11-13)

| Agent | Where its login is kept | Can one machine keep several logins? | Where conversations are kept |
|---|---|---|---|
| Claude Code 2.1.322 | macOS Keychain, one entry per config folder (`CLAUDE_CONFIG_DIR`); `.credentials.json` in that folder on Linux | Yes: one folder per account, documented by Anthropic | `projects/` inside the config folder |
| Codex 0.167.2 | `auth.json` in `CODEX_HOME`, or the system keyring if `cli_auth_credentials_store` says so | Yes: one `CODEX_HOME` per account | `sessions/` (plus SQLite indexes) inside `CODEX_HOME` |
| Antigravity `agy` 1.5.8 | System keyring, one entry for the whole machine (service `gemini`, account `antigravity`) | No. There is no folder or variable that moves it (antigravity-cli issues 155 and 381, open) | `~/.gemini/antigravity-cli/conversations`, shared by every login |

Sources: Anthropic, "Log in with multiple accounts" and "Credential
management" (code.claude.com/docs/en/iam); OpenAI, `CODEX_HOME` "sets the root
for Codex state, including config, auth, logs, sessions"; for Antigravity,
the agentsroom.dev write-up of the keyring entry and Google's two open
issues. A known Codex regression (openai/codex#50859) only affects swapping
`auth.json` under one `CODEX_HOME` while Codex's own TUI keeps a background
app-server; Sandhill starts its own `codex app-server` per conversation, so a
different `CODEX_HOME` is a different process.

## The approach

### Accounts are folders Sandhill makes, logins stay the agent's own

- **The existing login is the first account**, unchanged: `~/.claude`,
  `~/.codex`, Antigravity's keyring entry. Nothing about today's setup moves.
- **Each extra Claude or Codex account is a folder Sandhill creates** on the
  device where the agent runs, under
  `~/.local/share/sandhill/accounts/<agent>/<name>/`. The agent is started
  with `CLAUDE_CONFIG_DIR` or `CODEX_HOME` pointing there.
- **Signing in happens once, when the account is added** (Manage → Add
  Account…), in the agent itself: Claude Code's own login in a small
  terminal; Codex through app-server's sign-in, which the chat already
  offers. The agent keeps and renews that login in the account's folder, so
  switching later needs no sign-in. Only a login the agent can no longer
  renew (for example an account unused for a long time, or signed out on the
  web) asks again; the menu marks such an account "Sign in again". Sandhill
  never reads, copies or writes a token or a keychain entry, as the settled
  rules require.
- **Codex** gets `-c cli_auth_credentials_store="file"` for extra accounts,
  so each login stays in its own folder rather than one shared keyring item.

### One history, whichever account runs

So that the other account sees the same conversation, each extra account's
folder holds links back to the user's own folder for everything except the
login:

- Claude: `projects` (the conversations), `settings.json`, `CLAUDE.md`,
  `agents`, `commands`, `skills`, `plugins`, `todos`.
- Codex: `sessions`, `archived_sessions`, `config.toml`, `AGENTS.md`,
  `prompts`, `skills`.
- Never linked: Claude's `.credentials.json` and `.claude.json` (its login and
  first-run state), Codex's `auth.json` and its SQLite databases (two
  processes must not share one database file).

Conversations kept with a project (`.sandhill/agents/<id>/`, see
`chats.rs`) go on the same way after a switch: before a conversation goes on,
`prepare` puts the agent's copy back in the user's own folder, which every
account of that agent shares through the links. A switch adds nothing new to
carry: the project's copy keeps growing as before, whichever account wrote it.

The links live in Sandhill's own folder; nothing inside `~/.claude` or
`~/.codex` is created, edited or deleted by Sandhill. The agent itself keeps
writing its conversation files where it always does.

### Switching inside a conversation

- **Where**: the panel each agent's quota chip in the status bar opens. It
  lists that agent's accounts on the device, each with its last known 5-hour
  and weekly use, marks the one the shown conversation uses, and offers
  "Use in This Conversation" on the others, "Sign In" where a login is
  missing or expired, "Add Account…" and "Manage…" (Welcome → Manage →
  Agent Accounts: rename, remove). The chip's rings show the account the shown
  conversation uses, else the default one.
- **When the limit is hit**: the chat shows the agent's own limit message and,
  under it, "Continue with <other account>" for each account that still has
  room. Nothing switches by itself.
- **What switching does**: waits for the current turn to end (or asks to stop
  it), ends the agent's process, and starts it again on the same terminal
  with the other account, resuming the same conversation: `claude --resume
  <session>`, Codex `thread/resume`, `agy --conversation <id>`. The chat
  continues below a line "Switched to <account>"; nothing is lost from it.
- **Quota rings** follow the account in use; each account keeps its own last
  reading, so the menu can say which one has room.

### Antigravity

A second subscription login cannot be kept beside the first: the only ways are
signing out (machine-wide, every `agy` on the device follows) or rewriting the
keyring entry (reading and writing Google's token, which Sandhill does not
do). Two options are offered instead:

1. **API-key accounts** (recommended). An account is a Gemini API key, kept by
   Sandhill in the macOS Keychain under Sandhill's own entry and passed to that
   one run as `GEMINI_API_KEY`, with the key mode for that run only
   (`agy --settings`, which agy documents as "never saved"). These run side by
   side with the subscription and with each other. They bill the key, not the
   subscription.
2. **Change the Google login** for the whole device: a guided `/logout` and
   sign-in inside `agy`'s own screen, with a warning that every Antigravity
   conversation on the device changes account. Kept as a manual fallback.

### Devices and remote servers

Accounts belong to the device where the agent runs (this Mac or a server),
because that is where the folders and logins are. The Manage page lists them
per device, with Rename and Remove (Remove deletes Sandhill's folder and so its
login; the agent's own default login is never removed).

## To check first, before building the rest

Each is a short trial with a real agent, recorded in this plan:

1. Claude: a conversation started on the default folder resumes under a second
   config folder whose `projects` links back, and continues with full context.
   **Partly verified (2026-11-13, Claude Code 2.1.322):** in an isolated
   temporary `HOME`, a second config folder's `projects` symlink resolved
   a synthetic transcript to the exact same file as the original folder.
   The installed CLI documents `--resume <session-id>`, which Sandhill
   already passes. Authenticated continuation across two logins was not
   tested: no real login or credential was accessed. A fresh config folder
   in a real PTY first displayed “Welcome to Claude Code” and the theme
   picker, before sign-in. Sandhill therefore opens the full interactive
   first-run screen, rather than `claude auth login` alone; the user
   completes onboarding and signs in there, then exits that screen.
   No onboarding state is copied or written by Sandhill.
2. Codex: `thread/resume` by id finds a rollout through the linked `sessions`
   under a second `CODEX_HOME` (its own SQLite indexes start empty); if not,
   resume by the rollout's path, which the bridge already records.
   **Passed (2026-11-13, Codex 0.167.2):** an isolated temporary `HOME`
   started a real app-server thread, appended model-visible history and a user
   turn, then closed it. A second `CODEX_HOME`, with `sessions` and
   `archived_sessions` linked to the first and no SQLite files, resumed the
   same thread by ID. `thread/turns/list` returned the original user-context
   marker. The selected home's read-only `account/read` correctly reported no
   login. No real login, credential file or keychain entry was inspected;
   all agent configuration overrides were cleared and file auth was explicit.
   This checks persisted context, not an authenticated model response.
   A rollout-path fallback was not needed. The generated resume schema does
   not expose `path` in this version, despite mentioning it in its description.
   Sandhill now scopes account queries with `read_account_in(codex, home)`;
   an explicit home sets child-only `CODEX_HOME` and
   `-c cli_auth_credentials_store="file"`. Live and read-only processes also
   force file auth when `SANDHILL_CODEX_FILE_AUTH=1`. The current CLI
   device-login flag is `codex login --device-auth`.
3. Antigravity: `--settings` turns on key mode for one run without changing
   `settings.json`, and `--conversation` resumes a subscription conversation
   under a key.
   **Partly verified (2026-11-13, agy 1.5.8):** the exact per-run override
   is `--settings '{"modelProvider":"gemini"}'`; the key is inherited only
   through `GEMINI_API_KEY`, never passed in argv or protocol messages.
   Sandhill adds this override only for `SANDHILL_AGY_API_KEY=1`, to headless,
   native, model-list, usage and ACP-history processes, without replacing
   their existing conversation/model/permission arguments or writing settings.
   [Google's authentication reference](https://antigravity.google/docs/install#using-a-gemini-api-key)
   confirms that the provider setting is required; the environment alone
   has no effect.

   The installed CLI was run with a temporary `HOME` and an allowlisted
   environment (no inherited agent configuration overrides). A macOS sandbox
   denied network access, Security services and the real home, except the
   executable itself. No real key, login, credential or keychain was used.
   With no key, the override exited 1 naming the missing `GEMINI_API_KEY`;
   settings were byte-identical. With a dummy environment value and empty
   stream-json stdin, startup exited 0 and emitted a conversation ID.
   Resuming that ID with the same override and empty stdin exited 0 and
   emitted the same ID. This verifies API-mode initialization/resume, **not**
   a model response or subscription-to-key continuation with full context.
   That authenticated trial remains unperformed.

   **Persistence limitation:** on successful startup agy itself normalizes
   an existing settings file, even though the override is never saved.
   The probe's compact file was reformatted and its default
   `modelProvider:""` removed; `"gemini"` did not appear in the saved file.
   The normalized file then stayed byte-identical across another API-mode
   startup/resume and the rejected print-mode `/logout`. This CLI-owned rewrite is
   [documented](https://antigravity.google/docs/settings#how-the-cli-reads-and-saves-settingsjson);
   unconditional byte preservation cannot be promised. Sandhill does not
   read or write the settings file. API-mode `/usage` returned success with
   an empty `groups` array, not subscription quota.

   ACP history additionally requires sequential `initialize`,
   `authenticate` with `{"methodId":"gemini-api-key"}`, then `session/load`;
   the dummy-key, offline trial succeeded. Skipping `authenticate` returned
   `Authentication required`, so the bridge selects that method explicitly
   for added key accounts, without putting the key in the request.

   **Whole-device login change:** launch `agy` normally, then have the user
   type `/logout` in its prompt. There is no verified direct-launch logout:
   `agy /logout` was rejected as an unexpected argument (exit 2);
   `agy --print /logout --output-format json` explicitly rejected credential
   clearing in print mode (exit 2); `agy --prompt-interactive /logout`
   sent `/logout` as a model prompt instead of executing the slash command.
   The last behavior was observed in the isolated API-mode TUI after its
   onboarding/trust screens; the sandbox blocked its model request.
   Do not use that startup prompt for sign-out. Actual machine-wide sign-out
   and subsequent Google sign-in were deliberately not exercised.

If a trial fails, that agent's part is changed and brought back to Huan.

## Terms

Use only accounts you own. Anthropic's and OpenAI's terms forbid sharing a
login with anyone else; we found no limit on how many accounts one person
holds. A Google forum answer indicates alternating two paid accounts you own
is not itself a violation, but that is not a documentation page. Sandhill does
not pool, rotate or proxy accounts on its own; switching is always the
user's click.

## Questions for Huan

1. **Where the switch is**: decided: the status bar quota panel.
2. **At the limit**: offer "Continue with <account>" buttons (recommended), or
   switch automatically to the account with most room?
3. **Antigravity**: API-key accounts plus the guided whole-device sign-in
   (recommended), or only one of the two?
4. **Shared between accounts**: conversations, settings, instructions, skills
   and MCP servers, everything but the login (recommended), or conversations
   only?
5. **Remote servers**: accounts per device as above (recommended), or this
   Mac only for now?
