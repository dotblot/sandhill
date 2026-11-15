# Tidying files and folders

Huan, 2026-11-15. Status: approved, all five parts. Huan's answers: the
topic files are worth helpers having to open them; the program inside
the app is renamed `sandhill` as well; the six folder names stand.

Huan: "The framework and most of the details are fixed. No big changes in
the future." So the layout can now be made to fit what Sandhill became.
Huan asked for all five parts below.

## Rules for every step

- **Moving and renaming only.** No behaviour, wording, layout or look
  changes. A function reads the same before and after.
- One part at a time, one commit per step, each after the usual checks
  (`cargo fmt`, clippy, tests; for the app, inside its folder).
- The UI consistency work (`ui-consistency-plan.md`) pauses until parts 3
  and 4 are done, so the two never touch the same files at once.
- After parts 3 to 5: open the app once and click through Welcome, a
  notebook run, the Console, Git and the AI pane.

## Part 1: finished plans move to `docs/design/done/`

Stay at the top of `docs/design/` (still in use):

- `ui-consistency-plan.md` (in progress), this plan, and `ui/`.
- `bioinfo-ide-framework.md`, `agents-v2-plan.md`, `gpui-plan.md`:
  `CLAUDE.md` names them as the main design documents.

Move to `docs/design/done/` (built; kept as history): the other 24 plans
(`agent-accounts`, `close-file`, `completion-console`,
`data-explorer-redesign`, `file-opening`, `file-split`, `folder-chooser`,
`folder-trust`, `idle-agents-welcome-window`, `irkernel-install`,
`markdown-images`, `new-conversation`, `open-files-polish`, `openscience`,
`pi-omp`, `plot-save-window`, `rstudio-positron-borrowing`,
`server-split`, both `ui-feedback-2026-11-11`, and the four `welcome-*`).

Built 2026-11-15. `bioinformatics-assistance-plan.md` stays at the top:
its status says the PBMC3k end-to-end check was still running and its
guides await a domain expert's review, so it is not finished. Every link
to a moved plan (code comments, `CLAUDE.md`, `docs/design.md`, the UI
spec, the app's notes, the two plans that name each other) was updated,
and a check found no link to a missing plan. `done/README.md` says these
are history and may use old names.

## Part 2: `CLAUDE.md` split by topic

`CLAUDE.md` is 478 lines; "Decisions that are settled" alone is 400, with
some single bullets 80 lines long. It becomes:

- `CLAUDE.md` (about 80 lines): who Huan is and how to work, the
  conventions, and a list: "before touching X, read `docs/decisions/Y.md`".
  `AGENTS.md` stays a link to it.
- `docs/decisions/`, one file per topic, the same sentences moved, each
  under a short heading so a rule can be found and corrected:

  | File | Holds (today's bullets) |
  |---|---|
  | `look.md` | Look, tabs and capsules, consistency, popups and dropdowns, icons |
  | `window.md` | Window layout, title bar, Welcome, panes' size buttons |
  | `editor-and-kernels.md` | Editor row, kernels, Console, completion, Help, data explorer, notebook cells |
  | `files.md` | Notebook formats, standalone files, previews, compress/extract |
  | `agents.md` | AI conversations, agents, accounts, model and effort pickers |
  | `devices.md` | Daemon, wire protocol, quitting, closing files, device management, folder trust, journaling |
  | `licences.md` | Open source, Positron/Ark, brand marks, fonts |

  Nothing is reworded or dropped; a check counts that every sentence of
  today's file is in exactly one new file.

One cost to know: AI helpers are handed `CLAUDE.md` automatically, but the
topic files only when they open them. The list in `CLAUDE.md` tells them
which to read first; a helper that skips it could miss a rule. The gain is
that Huan can read and correct one topic at a time.

Built 2026-11-15. `CLAUDE.md` is 100 lines. The check found all 204
sentences of the old section in exactly one new file, with no word lost
or added; the only change is six new bullet dashes where one 80-line
bullet (editor, kernels, Plots, completion, explorer) was divided by
subject. Brand marks and fonts stayed with the icons in `look.md`, as
they were one bullet. The design-docs list now says `docs/decisions/`
wins over older plans, and lists `done/`.

## Part 3: the Mac app's folder is named `app/`

`gpui/` is named after the toolkit. It becomes `app/` (free since the
Tauri app was retired).

Changes with it: the root `Cargo.toml`, `notices/make.sh`,
`docs/design/ui/brand/build.py`, `README.md`, `CLAUDE.md`,
`docs/design.md`, `docs/protocol.md`, three comments in the daemon, the
app's own `README.md` and scripts, and the GitHub build recipe
`.github/workflows/gpui-mac.yml`, which is renamed `mac-app.yml`.
Finished plans in `done/` keep the old name (they are history).

The program inside the app is renamed too (Huan's answer): it is
`Contents/MacOS/sandhill`, built from the Cargo package `sandhill-app`.
The toolkit's own names (`gpui-kit`, `vendor/gpui-component`) are theirs.

Built 2026-11-15. Also changed: the app's "remove Sandhill" check of its
own bundle looks for `sandhill`, the bundle's launch check stops counting
`sandhilld` (whose name begins the same), the old Tauri lines left in
`.gitignore` went, and `docs/decisions/look.md` notes the new names.
`docs/design/gpui-plan.md` and the 2026-11-01 note in `docs/design.md`
keep `gpui/` as history. Checked: the app's 291 tests, the daemon test
that reads `app/src/imports.rs`, a release bundle that starts. The
GitHub recipe is proven only by a push. One thing to know: the
command-line client in `crates/cli` is also called `sandhill`; it is not
in the app bundle, and the two never share a folder.

## Part 4: the app's 84 loose files go into folders

Today `src/` has 84 files side by side. After: 7 at the top and 6 folders
named after the part of the window they draw. Files that repeat their
folder's name lose the prefix (`agents_view.rs` becomes `agents/view.rs`).

Stay at the top: `main.rs`, `workbench.rs` (+ `workbench/`), `view.rs`
(+ `view/`), `look.rs`, `prefs.rs`, `daemon.rs`, `connection.rs`.

| Folder | Files moving in |
|---|---|
| `agents/` (14) | `agents.rs` stays as its head; `agents_box` → `input_box`, `agents_start` → `start`, `agents_view` → `view`, `agent_accounts` → `accounts`, `agent_edits` → `edits`, `chat`, `composer`, `composer_view`, `effort_bar`, `quota`, `cell_ai`, `notebook_ai`, `ghost` |
| `editor/` (27) | `notebook`, `cells`, `cell_view`, `documents`, `render_document`, `statements`, `r_keys`, `rainbow`, `colouring`, `bio_text`, `find`, `outline`, `language/`, `output_text`, `output_table`, `html_table`, `table`, `table_selection`, `shown`, `md_images`, `doc_text`, `delimited`, `media` (+ `media/`), `pages`, `export`, `imports`, `import_dialog` |
| `panes/` (15) | `console`, `terminal`, `jobs`, `machine` (Kernels and resources), `kernel_menu`, `irkernel`, `help`, `packages`, `files`, `tree`, `tree_actions`, `plots` (+ `plots/`), `viewer`, `explorer` (+ `explorer/`), `object_explorer` |
| `git/` (5) | `git.rs` stays as its head; `git_diff` → `diff`, `git_log` → `log`, `git_pane` (+ folder) → `pane`, `git_rewrite` → `rewrite` |
| `welcome/` (6) | `start_screen` (+ folder), `folder_dialog`, `trust`, `resource_manager`, `app_uninstall`, `project_settings` |
| `shell/` (11) | `app_window`, `quit`, `main_menu`, `command_palette`, `focus_mode`, `popup`, `panes.rs` → `pane_sizes`, `meter`, `archive_tasks`, `close_file`, `download` |

How: each folder is one commit. Inside the code, a file's address changes
with its place (`crate::chat` becomes `crate::agents::chat`); those lines
are updated by the same commit and the compiler proves none was missed.
No forwarding names are left behind. `gpui/README.md`'s file names are
updated to match.

Built 2026-11-15, one commit per folder, each checked with the app's
format, lint and 291 tests. Two changes from the table: `console.rs`,
the workbench's own child (it reads the workbench's private fields),
went to `workbench/console.rs` instead of `panes/`, and `quit.rs` to
`shell/app_window/quit.rs`; both had been children kept elsewhere by a
`#[path]`, which is gone. `editor/`, `panes/`, `shell/` and `welcome/`
each have a short head file declaring the folder. `main.rs` reaches
`app_window` and `main_menu` through `use shell::{…}`. The app's README
gained a "Where the code is" table with the old names for its older
notes, rather than rewriting dated history; current docs and comments
were updated.

## Part 5: the few oversized files are split

Only files over 1,500 lines, split the way `file-split-plan.md` did it
(child files that can still see their parent's private parts; code moves
unchanged; tests move with their code):

- App: `notebook.rs` (2,035), `language/providers.rs` (2,024),
  `agents.rs` (1,903), `chat.rs` (1,858), `agents/view.rs` (1,687),
  `explorer.rs` (1,553), `agents/input_box.rs` (1,529).
- Daemon: `git.rs` (1,734), `lifecycle.rs` (1,696), `server.rs` (1,694),
  `terminal/antigravity.rs` (1,545).

Where each one is cut is decided by reading it, so before each split its
table (new file, what it holds, about how many lines) is added to this
plan for Huan to see. A file that turns out to be one long thing that
doesn't divide naturally is left whole and noted here.

### Daemon `git.rs`

Keeps the shared types, command runner and Git pane entry points.

| New file | What it holds | About how many lines |
|---|---|---|
| `git/status_read.rs` | Status and numstat parsing, untracked line counts, and their tests | 220 |
| `git/history_read.rs` | Log, refs and changed-file parsing, own-commit reading, and their tests | 250 |
| `git/action_helpers.rs` | Applying selected lines, action validation, conflict stops and rewrite helpers | 180 |
| `git/message_helpers.rs` | Agent prompts, invocation, message parsing, and their tests | 180 |

### Daemon `lifecycle.rs`

Keeps ownership/resource types and the daemon's lifecycle entry points.

| New file | What it holds | About how many lines |
|---|---|---|
| `lifecycle/ownership.rs` | Process inspection, ownership refresh and their tests | 330 |
| `lifecycle/components.rs` | Component manifests, MCP registration helpers, uninstall and their tests | 300 |
| `lifecycle/storage.rs` | Descriptor-relative storage access, accounting, removal and their tests | 340 |

### Daemon `server.rs`

Keeps daemon state, opening/serving and the complete call-dispatch match.
The earlier `server/` call modules stay in place; no match arms are extracted.

| New file | What it holds | About how many lines |
|---|---|---|
| `server/connections.rs` | Client handling, subscriptions, event forwarding, reply flushing and their tests | 490 |
| `server/request_helpers.rs` | Account reports, Git credentials, language server startup and trust checks | 230 |
| `server/session_helpers.rs` | Session lookup, quiet execution, redraw and job-awake helpers | 140 |

### Daemon `terminal/codex/mod.rs`

Keeps bridge state, process startup, account queries and the public entry points.

| New file | What it holds | About how many lines |
|---|---|---|
| `terminal/codex/commands.rs` | Chat commands, turn submission, interrupts, login and plan decisions | 275 |
| `terminal/codex/events.rs` | RPC reading, notifications, items/replies, approval requests and chat reports | 500 |

### Daemon `terminal/antigravity.rs`

Keeps state, startup, model/account settings, queued commands and public entry points.
The existing history, native-terminal and usage children stay in place.

| New file | What it holds | About how many lines |
|---|---|---|
| `terminal/antigravity/turns.rs` | Turn submission/execution, streamed events, tool/error translation and their tests | 650 |

Built 2026-11-15. All five daemon parents are under about 1,000 lines;
nothing was left whole. The server's single dispatch match remains intact.

### App `editor/notebook.rs`

Keeps document types, parsing, serialization and the main notebook entry points.

| New file | What it holds | About how many lines |
|---|---|---|
| `editor/notebook/quarto_cells.rs` | Lossless Quarto cell editing and source-view conversion | 440 |
| `editor/notebook/tests.rs` | Notebook parsing, editing, saving and kernel-selection tests | 710 |

### App `editor/language/providers.rs`

Keeps provider state, installation, requests and language-service entry points.

| New file | What it holds | About how many lines |
|---|---|---|
| `editor/language/providers/completion_items.rs` | Completion mapping, documentation, ranking, triggers, snippets and acceptance helpers | 700 |
| `editor/language/providers/completion_items/tests.rs` | Completion-item helper tests | 380 |

### App `agents.rs`

Keeps agent types, state, construction, refresh and conversation/account queries.

| New file | What it holds | About how many lines |
|---|---|---|
| `agents/terminal_events.rs` | Terminal subscriptions, output and chat event handling | 260 |
| `agents/view_switching.rs` | View handoff state methods, switching and legacy bridge recovery | 280 |
| `agents/messages.rs` | Starting conversations, submitting messages, interrupts and question answers | 380 |




## Order

1. Part 1 (plans), Part 2 (`CLAUDE.md`): documents only.
2. Part 3 (`app/`), then a push to prove the GitHub build.
3. Part 4, folder by folder.
4. Part 5, file by file.
5. The UI consistency work resumes.

## Questions for Huan

1. Part 2: are you content that helpers must open the topic files, in
   exchange for a `CLAUDE.md` you can read in a minute?
2. Part 3: keep the program's inner name `sandhill-gpui`, or rename it
   `sandhill` too? (Renaming touches the bundle recipe as well.)
3. Part 4: are the six folder names right? `shell/` means the window's
   frame; `frame/` or `window/` would also do.
