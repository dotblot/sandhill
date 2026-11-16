# Sandhill's Mac app

Sandhill.app, drawn with [GPUI](https://gpui.rs), Zed's UI toolkit. It
started as a trial (milestones 1 to 4 below); on 2026-11-01 it was chosen to
replace the first app, built with Tauri (then also in `app/`), and on
2026-11-02, once it did everything that app did, the Tauri app was deleted
(it is in the git history, last in commit d3ff9f6). This folder was `gpui/`
until 2026-11-15, and its program `sandhill-gpui`; notes below from before
then use the old file names. It kept the Tauri app's look, with fonts
2px larger, until 2026-11-05, when the window became frosted glass in
macOS's own colours (see the last entry below). "The Tauri app" in
comments here means that app.

It reaches `sandhilld` through
`sandhill-link` and `sandhill-proto`, and it borrows the daemon's own rules for
building outputs and finding Quarto chunks (`sandhill-daemon`'s `outputs` and
`qmd`). It is a separate Cargo project (its own `[workspace]`), so its many
dependencies stay out of the main build, its `Cargo.lock` and CI. UI pieces
come from `gpui-kit` (Apache-2.0); nothing is taken from Zed's GPL editor.

The current stack is GPUI Kit / Component / Base / WebView **0.9.1**,
GPUI Fast **0.1.15** and GPUI Pre **0.3.12**, locked together in `Cargo.lock`.
The latest released `gpui-cn` is still **0.8.0** and depends on Kit 0.8.1,
so `vendor/gpui-cn` retains that release's source, tests and licenses
(upstream commit `e49d909b2ceae040714aa53cd38f97faa4f85cd2`). Local changes
are limited to the Kit/WebView dependency pins, an isolated workspace for
maintenance checks, and constructing the newly non-exhaustive theme tokens
through their defaults. `Cargo.toml.orig` preserves the upstream manifest.
Do not patch the machine's Cargo cache.
When upstream supports the current Kit, compare and replace this local copy.

## Where the code is

Since 2026-11-15 (`docs/design/reorganise-plan.md`) `src/` holds the
core at the top and six folders named after the part of the window they
draw; each folder's head file (`agents.rs`, `editor.rs`…) declares it.

| Place | What |
|---|---|
| `main.rs`, `workbench.rs` (+ `workbench/`, with `console.rs`), `view.rs` (+ `view/`), `look.rs`, `prefs.rs`, `daemon.rs`, `connection.rs` | Starting up, a project window's state and drawing, styles, this Mac's settings, reaching sandhilld |
| `agents/` | The AI pane: conversations (`agents.rs`), `view`, `input_box`, `start`, `accounts`, `edits`, `chat`, `composer`, `composer_view` (also every popup's frame), `effort_bar`, `quota`, `cell_ai`, `notebook_ai`, `ghost` |
| `editor/` | Documents: `notebook`, `cells`, `cell_view`, outputs and tables, `language/` (completion), `media/` (previews), find, outline, imports |
| `panes/` | Kernels and resources (`machine`, `kernel_menu`, `irkernel`), Help, Packages, Files (`files`, `tree`, `tree_actions`), Plots, the Viewer, the data explorer, the Terminal, Jobs |
| `git/` | Git: `git.rs`, `pane` (+ `pane/`), `diff`, `log`, `rewrite` |
| `welcome/` | Welcome (`start_screen` + folder), `folder_dialog`, `trust`, `resource_manager`, `app_uninstall`, `project_settings` |
| `shell/` | The window's frame: `app_window` (+ `quit`), `main_menu`, `command_palette`, `choices` (this Mac's choices for every open project), `pane_sizes`, `focus_mode`, `popup`, `meter`, `archive_tasks`, `close_file`, `download` |

Notes below from before that date name files by their old places:
`agents_box`, `agents_start`, `agents_view`, `agent_accounts` and
`agent_edits` are now `agents/input_box`, `start`, `view`, `accounts` and
`edits`; `git_diff`, `git_log`, `git_pane` and `git_rewrite` are
`git/diff`, `log`, `pane` and `rewrite`; `panes.rs` is
`shell/pane_sizes.rs`; other files kept their names in their folder.

## Milestone 1: run one cell

- Starts (or finds) sandhilld on this machine.
- **Open Project…** picks a folder; its files show on the left.
- Clicking a `.ipynb`, `.qmd` or `.Rmd` shows its cells. The kernel is the one
  the document names, else the first for its language; the buttons above the
  cells switch it.
- **Run** on a code cell starts the kernel if needed and shows text output,
  warnings and errors. HTML shows a note until the web view (milestone 4).

## Milestone 2: edit cells and see plots

- Every cell is an editor (gpui-component's), with Python and Markdown
  colours built in and R from Posit's tree-sitter grammar (MIT).
- **Shift+Enter** runs the cell and moves to the next; **Run All**
  (⌘⇧Enter) runs them all in order.
- PNG, JPEG, GIF and SVG plots appear under cells, fetched from the
  daemon's blob store.
- Reopening a notebook shows each cell's latest run from the daemon's
  journal (or the outputs saved in an `.ipynb`), and carries on in the
  kernel that ran them if it is still running. Switching notebooks leaves
  kernels running; choosing another kernel stops the old one.
- **Save** (⌘S) writes the file back: an `.ipynb` in nbformat's layout,
  giving old cells ids (nbformat 4.5); a Quarto document with only the cells'
  text replaced. A file changed on disk since it was read isn't overwritten.
  Switching to another notebook saves the edited one first.

## Milestone 3: Console, Variables, Terminal, ssh

- The right side has **Console** and **Terminal** tabs over a **Variables**
  pane.
- The Console follows the open notebook's kernel and shows its history,
  each run labelled with the cell (or agent) that sent it. Code typed below
  runs in the same kernel (Shift+Enter). With no notebook open, selecting
  a kernel starts the Console's own session automatically; Shut down stops it.
- Variables lists the kernel's variables after each run; lists, data frames
  and other containers open with the arrow.
- Terminal is a real shell from the daemon (in the project folder), run
  through Ghostty's terminal engine (libghostty-vt) and drawn by gpui-cn's
  terminal element (`terminal.rs`; Huan's choice, 2026-11-02): colours,
  full-screen programs like `top`, Ctrl+C, copy and paste, selection,
  scrollback, links. Building it needs Zig 0.17 (`ZIG`, or `zig` on PATH)
  and network access to Ghostty's download hosts. It keeps running when the app closes and is picked up again.
- The **Machine** box at the top connects to another computer over ssh
  (`--host HOST` on the command line does the same), using the same
  `sandhill-link` code as the Tauri app. The start screen lists recent
  projects on that machine and takes a folder path.

## Milestone 4: web pages and the Data Explorer

- Web pages in outputs (htmlwidgets, plotly figures, anything with a
  script, large HTML kept as a blob) are drawn under their cells in a web
  view from `gpui-webview` (the system's WebKit, put into the window).
  Simple HTML such as a pandas table still shows as text.
- Pages shown from the Console go to a **Viewer** tab, with ◀ ▶ to go
  through them; it opens by itself when one arrives.
- The mouse wheel over a page scrolls the notebook unless the page uses
  it (a small script in each page sends the unused turns back).
- The app runs on GPUI Fast (`gpui-fast`), gpui-kit's newer drawing
  engine: on the older one a web view can't be clipped, so a page scrolled
  half out of the notebook covered the bars around it.
- **View** next to a data frame or matrix in Variables opens the Data
  Explorer in the middle: a table that draws only the rows in view and
  fetches them from the kernel 200 at a time as it scrolls. Clicking a
  column's name sorts by it (in the kernel). Tried with 1,000,000 rows.
- Known gaps: pages from files or addresses on another machine over ssh
  (they need a tunnel); filters in the Data Explorer; on Linux, keys go to
  the web view after a click elsewhere (a gpui-webview bug there); a page
  run again with the same HTML isn't reloaded.

## Step 1a: the Tauri app's look

From here on the work follows the porting plan (step 1: look and layout).

- `src/look.rs` adapts GPUI Kit's bundled Default Light/Dark themes for both
  Kit widgets and custom drawing. Only typography, geometry, existing surface
  opacity and Xcode syntax are overridden; interface colours and terminal ANSI
  hues come from Kit. It follows the system's appearance, not its accent;
  `SANDHILL_APPEARANCE=dark` (or `light`) picks an appearance for trying it.
- Every text size is the Tauri app's plus 2px: window text 15px, code 15px
  in Maple Mono NF, small rows 13px. Maple Mono is bundled as gzipped TTF in
  `assets/fonts` (GPUI reads TrueType, not the web fonts); `make.py` there
  makes them from the WOFF2 web fonts.
- Icons come from `docs/design/ui/icons` (Gruvbox Material, Lucide and
  Developer Icons, normalized by `build.py`), embedded and served as
  `sandhill/…`; file and folder matching rules remain in `registry.json`.
  Source revisions, licenses and notices are recorded under `icons/vendor/`.
- The window is glass panes 3px apart on the warm wall under a 31px title
  bar of capsules (on a Mac the title bar is Sandhill's own, with the
  window buttons at its left), the files pane directly on the wall (both
  replaced by frosted glass on 2026-11-05, below), a
  26px status bar. Cells have the Tauri gutter (#3, run count, Run under
  the pointer), code in a rounded box and outputs behind a line coloured
  by how the run went.
- Run All now runs cells in order: each run's request waits for the one
  before it (they used to race, so a cell could run before the one that
  defines what it uses).

## Step 1b: tabs, a file tree, panes that resize

- The files pane is a tree: each folder is read from the daemon
  (`list_dir`) the first time it is opened, folders first, with guide
  lines and the folder icons. `.git`, `.DS_Store`, `.ipynb_checkpoints` and
  `.Rproj.user` are left out, as in the Tauri app.
- Notebooks open in tabs. A notebook in a tab not shown keeps its kernel,
  runs and edits; runs started there keep arriving while another tab is
  shown. The Data Explorer is a tab too.
- The gaps between panes can be dragged (files | middle | right, and above
  Variables); an accent line shows under the pointer. The title bar's first
  two buttons hide and show the files pane and the right column.

## Step 1c: scripts and formatted Markdown

- `.R` and `.py` files open as scripts: one editor with line numbers.
  **⌘↵** (Ctrl+Enter on Linux), or **Run line**, runs the selection, or the
  statement at the cursor, and moves to the next statement, as in RStudio:
  a call over several lines, a pipe chain, a function or a Python block
  is one statement (`src/statements.rs`, the rules of the Tauri app's
  `statements.ts`). **Run file** (⇧⌘↵) runs it all. The runs show in the
  Console, journaled under the script (`console-0` to `console-99` in turn,
  as in the Tauri app), so reopening it carries on in the same kernel.
  Scripts run in the project folder and only in a kernel for their language.
- A notebook's Markdown cells show formatted (headings at the Tauri app's
  sizes); a double click edits one, and Shift+Enter or clicking away shows it
  formatted again. A Quarto document's prose stays text, as in the Tauri app.
- In a notebook, ⌘↵ runs the cell and stays on it.
- **Code intelligence** (`src/language/`): focus or edit an R/Python editor to
  attach a daemon language service. Python uses ty plus ruff; R uses the
  document's Ark session. Python hosts are shared per project; R hosts are shared
  only by documents using the same Ark session. Notebook/Quarto code is mapped to
  a stable file URI per language, with blank separators between code cells.
  Full-text changes wait 300ms; completion flushes changes first. Diagnostics
  update after 500ms idle, with UTF-16 wire columns mapped back to cell columns.
  Completion merges live kernel names first (ipykernel/IRkernel), deduplicates
  labels, and uses Ark's LSP instead of its empty Jupyter completion reply.
  Console input uses kernel completion only. Hover and definition use the same
  daemon channel, locally and over SSH. A muted action-row status explains an
  unavailable service without interrupting typing; an exited service restarts
  on the next edit/focus. Closing or renaming releases documents and detaches a
  language host when its last document closes.

## Step 2: Agents

The AI pane, as the Tauri app has it (`AgentsPane.tsx`, `AgentChat.tsx`,
`AgentComposer.tsx`, `agentEdits.ts`), text 2px larger.

- **2a** (`agents.rs`, `agents_view.rs`, `chat.rs`): conversations are the
  project's, not a file's. The chat shows your messages, the agent's
  replies, its steps (read, edit with a diff, bash) and what it asks; the
  terminal button shows the agent's own terminal. History lists past
  conversations by day; New picks Claude Code, Codex, Antigravity or Posit
  Assistant, whose conversations show `pa`'s own screen instead of a chat.
- **2b** (`composer.rs`, `composer_view.rs`, `agents_box.rs`): the message
  box. `@` adds files and variables of the project's running sessions, `#`
  adds cells of open notebooks, each as a chip that goes with that message
  only; files from this computer can be attached. Menus for the model
  (favourites, each agent's models, thinking effort), the mode (Posit's
  names) and where agent code goes, and the context ring with Compact now.
  Each conversation keeps its own unsent text.
- **2c**: an agent's questions (one or many answers, Other), plan approval
  (carry it out, allow edits, keep planning) and command approval; a step
  that touched a file has an Open link.
- **2d** (`agent_edits.rs`): the window tells the daemon where you are (the
  file in front, its cell, the selection, open files). When an agent adds or
  changes a cell in a notebook open here, the window makes the edit in the
  editor, saves and runs it, so you see it happen. The gear menu compacts,
  renames (or double-click the title) and ends a conversation; an ended one
  picks up again when you send a message, or starts afresh like it.

Not yet: ⌥1–9 model shortcuts, plots in `@`, pasting or dragging files in,
exporting a conversation, and a cell added by an agent clears that
notebook's undo history.

## Step 3: right-side panes

The panes sit as in the Tauri app (`App.tsx`): the Console, Terminal and
Jobs under the editor (⌘J shows or hides them); on the right, Variables,
Help and Packages over Plots and the Viewer.

- **3a** (`panes.rs`, `plots.rs`): the Plots pane keeps every plot the
  Console, scripts and agents draw (notebooks' stay under their cells), read
  back from the daemon when the window opens. Previous and next, thumbnails,
  where each came from and when; zoom (double-click) shows it large and
  saves it as PNG, SVG or PDF, drawn again at that size when the session
  can (R; Python from its figure), asking before it replaces a file.
- **3b** (`help.rs`, `packages.rs`): Help searches the session's help (R's
  installed packages, Python's imported modules) and shows the page, with
  links, Back and Forward, the function's source and runnable examples.
  `?lm`, `help(lm)` and `np.mean?` at the Console open here. Packages lists
  what's installed, attached or imported first, with a filter; a click opens
  the package's help; install (pip or conda in Python, BiocManager or CRAN
  in R), update (R checks CRAN and Bioconductor when asked) and remove run
  in the Console, and the list is read again after.
- **3c** (`jobs.rs`): Jobs runs a command (New job) or a script (Run as job,
  above a saved script) in the background on the daemon's machine, each with
  its log, how long it has run, its CPU and memory; stop it, run it again or
  remove it. Jobs keep running when the window closes.

Not yet: plots fitted to the pane (they are drawn once, at the size R or
Python chose), copying a plot, Esc to close the zoom, R's vignettes and
LaTeX in Help, the session picker in Packages (it follows the Console's
kernel), and the file tree doesn't refresh when a plot is saved. R's help
pages weren't tried here (no R on the test machine).

## Step 4: Git

A Git tab next to Packages, as in the Tauri app (`GitPane.tsx`), reading
and changing the repository through the daemon (`GitStatus`, `GitLog`,
`GitShow`, `GitDiff`, `GitRun`), so it works the same over ssh.

- **4a** (`git.rs`, `git_pane.rs`): the branch, how far ahead or behind its
  upstream, and Staged and Unstaged files with their line counts and sizes;
  stage, unstage or throw away (asked first) a file or all of them; big
  files ask whether to stage or add to `.gitignore`. The commit box (Enter
  goes to the description, ⌘Enter commits) with Amend, "Write it" (the
  project's agent writes the message), and Commit and push or Stage all and
  commit. Fetch, pull, push and undo the last commit on the strip; switch,
  make, merge and rebase branches, tags and stashes from the menus. Merges
  and rebases that stop show a banner with Continue and Stop, and each
  conflicted file takes Mine or Theirs. "Start one here" makes a folder a
  repository; your name and email are asked once if Git doesn't know them.
  Passwords git or ssh ask for while pushing or pulling are asked for here.
- **4b** (`git_log.rs`, `git_diff.rs`): the history as a graph, with branch
  and tag badges and who made each commit (agents' commits marked);
  clicking a commit shows its message and changed files. A file's diff opens
  as a tab by the editor; click line numbers (Shift for a range) to stage,
  unstage or throw away just those lines, or a whole hunk.

Not yet: more than one diff tab at a time, dragging to pick lines, the
squash and reorder (rewrite) dialog, and pushing or pulling with a password
wasn't tried here (no remote that asks for one).

## Step 5: the hard parts

- **5a** (`explorer.rs`, `table.rs`): the Data Explorer as in the Tauri
  app. The columns are listed at the side with a small histogram or bar
  chart each, and the picked column's range, middle, commonest values and
  missing count under them; clicking a value filters to it. Filters (Add
  filter: a column, a condition, its values, or ticks for a category's
  values) show as chips to change or remove. Shift-click sorts by another
  column too. Copy as code writes the R or Python that does what the view
  does; Export CSV saves the rows shown, asking before it replaces a file.
  The table waits while code runs in its session and is read again after.
- **5b** (`daemon.rs`, `pages.rs`, `viewer.rs`): pages on a server's ports
  (Shiny apps, Quarto previews, the daemon's page server) come through the
  daemon's own connection, as `ssh -L` would, with no second login. Preview
  (⇧⌘K) above a `.qmd` or `.Rmd` runs `quarto preview` on the daemon's
  machine and shows it in the Viewer, rendered again on each save; Stop
  ends it. HTML files open in the Viewer from the file tree.
- **5c** (`project_settings.rs`, `import_dialog.rs`, `imports.rs`): the
  ☰ menu (the gear until step 6) opens Project settings (the R and Python the
  project runs in, where notebooks run, env files, where agent
  conversations are kept), saved in `.sandhill/project.toml`; a project
  without saved settings asks once. New notebooks follow the project's
  kernel and folder. Variables' Import menu reads a dataset into the
  session (text, Excel, SPSS/SAS/Stata, R data, Parquet, AnnData, 10x
  output) after showing the code it will run.

Not yet: keeping a table's filters and sorting with the project for next
time. Tried here without a server (the tunnel is checked against the
local daemon in `cargo test`) and without Quarto or R installed, so a real
preview and R's imports weren't seen.

## Step 6: switching over

- **6a** (`command_palette.rs`, `main_menu.rs`, `tree_actions.rs`,
  `files.rs`): ⌘K goes to a file of the project or runs a command, by
  typing part of its name. The Mac menu bar (Sandhill, File, Edit, View,
  Run, Window) and the Tauri app's shortcuts: ⌘N and ⌥⌘N, ⌘O and ⇧⌘O,
  ⌘S and ⌥⌘S, ⌘W and ⇧⌘W, ⌥⌘R, ⌘B, ⌥⌘B, ⌘I, ⌘J, ⌘K, ⇧⌘K and F1 (help for
  the name under the cursor). On Linux they use Ctrl, except in the
  terminal, where Ctrl keys stay the shell's. The title bar's ☰ menu has
  other projects and servers, project settings, the console, and whether
  notebooks' runs show in the Console and their plots in Plots (off, as in
  the Tauri app). The files pane's + menu, ⌘N and the right-click menu
  make files, folders, Quarto documents, R scripts and notebooks, named
  in a field in the tree; the right-click menu also opens, runs as a job,
  renames, duplicates, reveals, copies the path and moves to the Trash (on
  a server, deletes after asking). Quitting or closing the window saves
  edited documents first.
- **6b**: the Mac app carries Linux builds of sandhilld and Ark in
  `Contents/Resources/daemons`, which it installs on servers over ssh as
  the Tauri app does (`$SANDHILL_DAEMON_BUILDS` points elsewhere). About
  Sandhill shows the build.

## Step 7: filling the gaps

- **7a** (`app_window.rs`, `start_screen.rs`): a window originally held a
  tab per machine. Since 2026-11-06 it holds one independent capsule per
  project, identified by host and project path, so opening another project
  on the same device no longer replaces the first. Right-click closes a
  project, after asking. The windows and their projects open again at the next
  launch (`gpui-windows.json` next to `gpui.json`); closing the last
  window with projects shows a fresh Start window instead of quitting. The
  Start screen has a card for each server and this Mac with its recent
  projects (opened when, or kernels running; × forgets one), Open Folder…
  (a folder picker that browses any of them through its sandhilld) and
  Connect to Server…. ⌘O, ⇧⌘O, ⇧⌘N (New Project) and the title bar's +
  show it. The list of servers is the Tauri app's own file, so both apps
  share it. Window ▸ New Window opens another window.

- **7b** (`machine.rs`, `connection.rs`): the project's name in the left
  pane lists the machine's projects (with their running kernels) and
  switches to one. The Kernels tab lists the project's running
  sessions with what uses each and its memory; the trash button, and
  "Shut down N unused sessions", ask first. Resources shows the machine's
  CPU, memory, GPUs and disk as bars (now in the status bar); clicking one
  shows its details. The left status bar's device item opens a pop-up with what the
  connection is, Reconnect (the project and its documents open again),
  Disconnect, Connect, or Restart sandhilld on this machine, and the
  daemon's log. A document's kernel has Restart and Shut down beside its
  state. Closing a document with unsaved changes asks Save, Don't save or
  Cancel.

- **7c** (`download.rs`, `tree.rs`, `plots.rs`, `viewer.rs` and others):
  the files sort by name, size (folders' sizes too) or date modified, and
  a row's tooltip says its size and when it changed; a server's file can
  be downloaded to the Mac's Downloads folder. Variables has a filter box.
  The Viewer has Reload and Open in browser beside Back and Forward.
  Plots can be copied as a PNG, are drawn again by their session to fit
  the pane once it stops changing size, and Esc closes the large plot.
  The Data Explorer keeps each table's filters and sorting per project
  (`explorer.rs`), following a column that moved. Git's More menu has
  Squash or reorder commits (`git_rewrite.rs`) for commits not yet pushed.
  In the AI pane, ⌥1–9 picks a model while the model menu is open, images
  and files can be pasted or dragged into the message, `@` offers the
  sessions' newest plots, and the gear menu exports the conversation as
  Markdown, HTML or JSON into the project (`export.rs`, the same as the
  Tauri app's). Help's search lists R vignettes, which open in the Viewer
  as their package built them, with their R code a click away.

- **Retired the Tauri app** (2026-11-02): this is Sandhill.app now, with
  the Tauri app's bundle id (`dev.sandhill.ide`), so it replaces it on the
  Mac. The app icon's sources and its script moved to `assets/icon/`.
- **New layout** (2026-11-02, Huan's): the left panel holds Variables,
  Kernels, Help and Packages over Files, Git, Plots and the Viewer; the
  middle has the editor with the Console, Terminal and Jobs under it; the AI
  pane is the right panel; widths 1:2:1, 10 px from the window's sides.
  Dragging the gaps changes the shares (`panes.rs`). Resources are at the
  right of the status bar (`machine.rs`); the title bar's sun/moon button
  switches the theme and is remembered (`look::toggle_appearance`).
  Notebook cells drop the run count, have Run at their top right, and
  their output text is selectable (`SelectableText`); a plot under a cell
  can be saved or copied from the buttons over it (`plots.rs`,
  `open_cell_plot`).
  Cells have Positron-style tools, status and run times, add rows, keys
  for picked cells (Esc, then A/B, D D, Z, M/Y, arrows) and dragging by
  the gutter (`cell_view.rs`, `cells.rs`); data frames show as a grid
  (`html_table.rs`). See docs/design.md.
- **Frosted glass** (2026-11-05, Huan's): on a Mac the window is frosted
  glass, with the system blurring what is behind it under a neutral tint
  (`look::wall`, a `Blurred` window in `app_window.rs`), and the panes
  float on it: the editor solid (`look::pane`), the side panels, the AI
  pane and the Console group translucent (`look::side_pane`). The warm
  colours are gone for AppKit's greys and system colours, the accent is
  the one in System Settings (`look::system_accent`, read again when the
  window comes to the front), and code uses Xcode 27.2's Default (Light)
  and Default (Dark) colours. `cargo run` finds the daemon built at the
  repo root through `.cargo/config.toml`.
- **Big notebooks scroll smoothly** (2026-11-05): only the cells within a
  screen of the view are drawn; the others are space of their last height
  (`cell_view::Placed`). Scrolling a 190 MB notebook of 136 cells went from
  28 ms a frame (221 ms in `cargo run`) to the display's 16.7 ms. `cargo
  run` builds the dependencies optimised (`[profile.dev.package."*"]`).
- **Huan's notes, 2026-11-05**: a pointer resting on the 2px above or
  below a cell's outputs no longer stops the notebook's scrolling (the
  wheel wrapper covers the output box exactly, `view.rs`); the Console,
  Terminal and Jobs are translucent like the side panels; the title bar's
  edit capsule is gone; a cell's Run button is first in its tools, before
  Run the cells above; the status bar drops the machine and the key hints
  (Connection… moved to the ☰ menu); the title bar's tab shows the
  project's name only, and it and the status bar are in the text colour.
- **Small visual adjustments** (2026-11-05, Huan's): the Console input
  uses the AI composer's background and border colours, in light and dark.
  The Start screen and its project/server forms no longer show the inactive
  sidebar-button capsule beside the traffic lights. Notebook output boxes
  are inset 8 px on each side with a 1 px U-shaped border, square at the
  open top and rounded at the bottom, without recolouring their contents.
  The cell's Run button reuses the other tools' round hover background.
- **Notebook and pane refinements** (2026-11-05, Huan's approved plan):
  output side borders now connect to the code box and enclose run metadata;
  code cells have line numbers. Only call keyword names are italic in R
  and Python, through identifier-only `label` captures. Console and AI
  input frames are transparent with rounded borders. Four pane groups
  have Collapse and Expand vertically controls; their sibling keeps only
  its title bar, widths never change, and restoring retains the normal
  height shares. The Console close button is gone. Document loads are
  tracked through asynchronous reading/parsing, including failures and
  concurrent opens; a public AppKit cursor tracking area requests a clock
  badge while the shown workspace is loading (`busy_cursor.rs`). Huan
  confirmed that opening the large notebook displays the clock badge and
  completion restores the ordinary pointer.
- **Project capsules and flat titlebar controls** (2026-11-06, Huan's
  approved plan): each project has a separate capsule containing its name,
  a divider and its open document tabs. Only the active capsule shows full
  labels and close controls; others show file icons with hover paths.
  File tabs are no longer repeated over the editor. Same-device projects
  keep their own documents, Console and AI state; the left status connection
  item follows the active project. Left/right titlebar controls have no
  enclosing capsule. Window restore records use `(host, path)` selections
  and read older host-only records. Python import tokens and R
  library/require calls are italic without changing their existing colours.
- **Upstream icon libraries** (2026-11-06, superseding Atom Material and the
  neutral custom integration set): the 150 shared icons use Gruvbox Material
  for files/folders, Lucide for controls, and Developer Icons for available
  integration/runtime logos. Gruvbox colours are preserved without recolouring.
  The offline generator keeps existing IDs and file matching rules, preserves
  upstream SVG originals and licenses, and refreshes app assets, mockup and
  preview together. Generic substitutions are documented in `icons/MAPPING.md`.
  App bundles carry the icon license notices. Sandhill's crane, app icon,
  fonts, layout and actions are unchanged.
  Posit Assistant now uses the black mark from Huan's reference image,
  traced without the wordmark; Antigravity uses his supplied full-colour SVG.
  Their public-release permissions remain to be documented.
- **GPUI upgrade and model menu fixes** (2026-11-06): the GPUI stack is
  upgraded as above, retaining the Ghostty terminal through the local
  `gpui-cn` adaptation. Thinking effort uses Kit's standard stepped slider,
  without custom dots, a brain knob or explanatory prose. The model menu is
  fixed at 280×360px; only the model rows scroll, while tabs and effort stay
  in place. Antigravity now selects structured headless input explicitly:
  JSON output alone had left the official CLI trying to open its TUI.
- **Approved file/UI extension** (2026-11-07): effort levels now have labelled
  tick marks beside Kit's standard slider. Dropdown triggers ignore their own
  outside-mousedown so a second click closes instead of reopening the popup;
  switching triggers, outside clicks and Escape dismiss the previous menu.
  Files' New menu includes Python (`untitled.py`). Project capsules prefix
  the name with the local-device/server icon, and Welcome has no project `+`.
  Standalone Markdown (`.md`, `.markdown`, `.mdown`, `.mkd`, case-insensitive)
  toggles between source and a rendering of the current edits in the same tab.
  Shell extensions and recognised extensionless shebangs use Bash highlighting.
  Neither document type starts or attaches an R/Python kernel on opening.
  CSV/TSV first asks for editable text or import into a live R/Python session.
  Text preserves the original source, colours adjacent columns and pins the
  first logical record with a grey background. Text opening is limited to
  5,000,000 bytes, enforced by bounded daemon byte reads as well as the dialog.
  Import uses the existing code preview and protects existing variable names.
  Unsupported files ask before handing off to the default Mac app; remote
  files are downloaded without overwriting an existing copy before opening.
  Native event smoke checks cover opening, editing, saving, dropdown toggling
  and real Python and R imports. Real-kernel import and byte-limit regressions pass.
  The verification Mac was locked, so screenshots and final visual inspection
  remain unavailable; a real remote SSH host was not exercised.
- **Unified Kit colours** (2026-11-08, Huan's approved plan): replaced the
  custom macOS interface palette with Kit's resolved default light/dark
  colours. Kit components, Base and custom panes now agree; primary and
  destructive controls use the corresponding contrasting foregrounds.
  Git lanes/avatar colours and Data Explorer category swatches use Kit
  chart/base hues. Existing visible and hidden terminals update immediately
  when appearance changes, retaining output and using Kit's sixteen ANSI hues.
  Glass/transparent inputs, layout, typography, icon source colours, Xcode
  syntax and its italic captures are unchanged. Native runtime checks covered
  both modes, existing terminal ANSI output, hidden-terminal updates, editor,
  TSV, Welcome/project and narrow/short-window redraws. The verification Mac
  remained locked, so final screenshot inspection was unavailable.
- **Console and pane simplification** (2026-11-08, Huan's): the Console no
  longer has a Start button; selecting its kernel starts it immediately.
  Re-selecting a ready/starting kernel does not duplicate it, while selecting
  it after shutdown retries. Rapid selections/cancelled startup discard stale
  results and stop their unused kernels. New conversation has no agent-button
  row; the composer provider/model picker retains every agent, including
  Posit Assistant, without exposing model/mode controls PA manages itself.
  Resource indicators follow the connection item at the left of the status
  bar; their details open above the corresponding indicator. Native mouse
  checks exercised Python/R selection and Console execution, rapid switching,
  cancellation, PA/provider switching, resource popups and 900×600 layout;
  normal and narrow-window screenshots were inspected.
- **Delimited compatibility, quit and file sizes** (2026-11-08): text opening
  uses the original `read_bytes` protocol, not a new RPC missing from older
  servers. Metadata does not rely on their optional `stat_files.size`.
  Bounded chunks preserve UTF-8 across boundaries and reject size/version
  changes, malformed replies and files above 5,000,000 bytes. Import choices
  show readable environment/session/folder labels without internal IDs.
  Every file row displays its size, including supported formats and empty files.
  Quit asks about connected background services, defaulting to No; edits must
  save first, and a conflict prevents both exit and stopping. Explicit Stop
  leaves kernels, terminals and jobs alive. Native checks covered legacy-wire
  CSV/empty-TSV opening, the disabled oversized Text choice, real second-R-session
  import, quit cancellation, native termination, default Return and save conflicts.
  Closing a project window returned to Welcome; its quit prompt retained the
  prior connection. Two projects on one host produced exactly one StopDaemon
  request; the owned daemon exited and all three real kernels survived and
  reattached. Screenshots were inspected; CSV compatibility with a real remote
  SSH server was not exercised.
- **Crane navy, compact editor and media** (2026-11-08, Huan's approved seven
  items): Kit widgets and custom drawing share navy/light-blue interaction
  colours while neutral glass, Xcode syntax, status colours, icons, charts and
  ANSI colours stay unchanged. Editor actions and size controls share one row,
  including media and Git diff; overflow actions scroll with size controls pinned.
  Kernel dots replace status phrases, inside the editor picker and after the
  Console filename (inside its picker without a file). The picker lists only
  runnable kernels, including conda/mamba kernels; the separate environment
  inventory was removed at Huan's request.
  Last document choice is saved before execution and restored after reopening
  or app restart, behind an existing live session and ahead of project defaults.
  Missing choices remain explicit, never silently changing environments.
  Cell code/output borders strengthen together without a gutter stripe or
  layout shift; text outputs use I-beam styling and retain drag-copy.
  Transparent Console/AI inputs have clearer inactive borders.
  PDFKit and AppKit provide middle-tab PDF/image previews, fit/zoom, PDF
  pagination and GIF animation. SVG is inert and external references are denied.
  File bytes stream through the existing daemon page server/port tunnel into
  private temporary caches; closing tabs/projects hides/removes native views.
  Native smoke inspected light/dark, 900×600 layout, text copy, cold kernel
  recovery, missing choice, PDF page/fit/zoom/resize, every supported image class,
  animated GIF frames, escaped paths, corruption, tunneled transfer, project
  switching, collapse and the real Git diff header. No real remote SSH host
  was used for these preview checks.
- **Vertical model providers and live selection** (2026-11-08): the model menu
  is fixed at 380×360px; its left provider rail is now 32px and icon-only,
  with names in hover tooltips. Favorites stays first, then providers sort alphabetically; the right
  model list scrolls and effort ticks fit that column. New conversation initially
  uses Antigravity's first returned actual ID, currently Gemini 3.8 Flash High.
  Returned sibling effort variants share one row, and the slider selects the
  corresponding real model without a contradictory separate effort argument.
  Existing selections, raw favorite IDs and running/resumed chats remain coherent;
  browsing another provider hides the unrelated current-provider effort scale.
  `output_text.rs` uses the standard selectable-text implementation with a local
  retained owner subscribed to selection changes, avoiding global refresh/timers.
  Native checks observed highlight changes during held, reverse, multiline and
  scrolled Unicode selections, plus copy and shared table rendering. The original
  release-only symptom did not reproduce with the local short-file baseline, so
  there is no observed failing-before claim. The previously supplied large
  notebook volume was unavailable. Actual CLI listing, Low/Medium/High ID routing,
  provider switching, six-level ticks and light/narrow layout were exercised
  without sending a model prompt.

- **Local bundles and kernel-picker rollback** (2026-11-08): local test bundles
  include matching static Linux daemons for x86_64 and ARM64. Both local builds
  and the Mac build workflow use `build-server-daemons.sh`; the existing SSH
  checksum/install/attach path can replace an older daemon on reconnect.
  At Huan's request, the kernel picker returns to runnable kernels only,
  grouped by language. Removed the separate environment inventory, its query,
  error/disabled rows, protocol type, server handler and inventory-only tests.
  Conda/mamba kernel discovery, command deduplication and remembered document
  choices remain. Icon-only AI provider tabs and packaged Linux builds remain.
  Native smoke verified the R/Python-only menu with actual local kernels and
  selection starting the `omicverse` Python kernel, with no environment rows;
  the real kernel executed `print(6 * 7)` and the Console displayed `42`.
- **Device lifecycle and storage management** (2026-11-08): the status-bar
  device menu and Welcome screen open Resources and storage. Select devices
  explicitly (current device by default), inspect affected projects/resources,
  category sizes and exact paths, then confirm Stop All, cleanup or uninstall.
  Force is a separate action after graceful shutdown leaves resources running.
  Stop All ends owned kernels/terminal hosts/jobs/agents before the daemon;
  default Quit keeps work, alternate Quit uses the same Stop All semantics.
  After stopping a device, Read selected inventories explicitly restarts only
  its daemon so storage can be inspected or cleaned; it does not restart
  kernels, terminals or jobs.
  Save failures, unreachable devices and unconfirmed processes prevent success.
  Storage cleanup preserves history unless selected; project `.sandhill` paths
  remain protected. Local settings cleanup is separate and closes the app to
  prevent recreating settings. Preview caches use private leased session
  directories, protect active previews and bound orphan files to 256 MiB,
  128 files and seven days. Legacy untracked temporary files are not adopted.
  Uninstall checks exact installation manifests/checksums and exact owned
  Antigravity MCP entries, leaves modified/unowned components reported, and
  preserves backend data by default. Optional current-App Trash occurs last,
  only after selected device uninstalls succeed. Development executables are
  never treated as removable app bundles. An uninstall tombstone blocks stale
  automatic restarts; explicitly reinstall before reconnecting that device.
  Older untracked processes/components and reattached macOS kernels without
  safe retained process handles may require manual intervention; those appear
  as remaining rather than being force-killed by a guessed PID or filename.

- **Legacy SSH upgrades and status-bar management fixes** (2026-11-08):
  pre-manifest daemon/Ark installations upgrade after preserving their exact
  bytes at `<target>.legacy-sha256-<checksum>`. Backup paths are reported;
  conflicting backups and modified recorded installations remain protected.
  Opening Resources and storage from a Workbench defers the window update until
  the click handler releases its entity lease. A permanent GPUI runtime test
  covers the original double-borrow panic. Native checks exercised the actual
  local/remote status-bar menu, including a disconnected remote panel; a real
  Ubuntu upgrade verified the legacy backup checksum and preserved existing
  kernel/terminal process birth identities.

- **Compact status, account quotas and replay-safe terminals** (2026-11-08):
  connection state is dot-only; Console controls share its tab strip, and outer
  side-panel margins are 7px. Host CPU/memory/disk/GPU meters are static rings
  with rootless thermal details and explicit unknown/error states. Linux
  NVIDIA/DRM and public macOS IOKit sources retain per-device measurements.
  Right-hand provider rings show official CLI account quota windows/reset times,
  including with no project or hidden AI pane. Codex snapshots and Claude
  statusline measurements are identity-bound; stale, expired or ambiguous
  readings do not imply zero use. Claude hooks verify identity at measurement
  time through the official CLI; no authentication token files are read.
  Terminal history is separately marked and reconstructs the screen without
  answering old queries into the live shell, including fragmented sequences.
  Native Mac and Ubuntu reattachment checks observed no replay-generated input
  and complete subsequent live DA/DSR/OSC/version responses. Tests also cover
  old host replay boundaries, quota/account transitions, unsupported hardware
  and fork-inherited preview-cache lease release.

- **Resizable plot zoom** (2026-11-09): drag the enlarged plot's bottom-right
  corner to change its width and height independently. The original R/Python
  session redraws at the preview size after 400 ms of stable geometry, rather
  than scaling a bitmap. Fixed plot aspect ratios remain intact. Resizes are
  coalesced and outdated replies cannot replace the current preview. Busy or
  unavailable sessions keep the displayed image and explain why; Export
  dimensions remain independent. Native pointer/resize checks used real Ark
  and matplotlib plots, including fixed aspect, delayed replies, busy kernels,
  plain images and ended sessions. Desktop screenshot inspection was unavailable
  because Screen Recording permission was not granted.

- **Workspace controls and retained Console sessions** (2026-11-10): each
  project name has a close X; inactive capsules combine same-type file icons
  with counts. Resource buttons use CPU, memory, GPU and disk icons.
  Welcome has the workspace theme switch and readable dark secondary text.
  AI quota details open on click, not hover; per-device provider switches
  control both status-bar visibility and periodic polling. Refresh now queries
  only that provider and bypasses cached values, even if its display is off.
  The account control remains reachable when every provider is hidden.
  Switching unbound Console kernels preserves their sessions and outputs;
  the kernel picker sits beside the pane controls. Command history shows the
  newest 50 commands, retains 100, and fills the input without executing.
  Trimming command history does not delete execution/output journal evidence.
  Stop-and-quit must confirm actual daemon exit, not just the stop reply;
  explicit reopen safely recovers a stopped daemon without restarting kernels.
  Verification used real Ark/Python kernels in an isolated project: switching
  while busy, switching back with variables/output intact, closing/reopening
  projects, history selection without execution, quota switches and manual
  refresh, click-only popups and grouped file icons. The app's own Metal
  renderer supplied normal/minimum-size light/dark frames; desktop blur was
  not captured. Narrow protocol, lifecycle, provider, replay/history and icon
  grouping checks passed. The owner's Ubuntu was not stopped or used as a
  destructive test target; matching Linux payloads were built separately.

- **Ubuntu reopen and Antigravity usage corrections** (2026-11-10):
  legacy maintenance recovery cooperatively replaces only the daemon, even
  when its inventory contains unverifiable independent work. It still waits
  for active maintenance and confirms the old daemon's exit. Actual Ubuntu
  reconnect was verified with existing Terminal/Codex birth identities intact;
  the regression fails with the former empty-inventory prerequisite.
  Antigravity quotas now come from the official JSON `/usage` built-in,
  retaining Gemini and Claude/GPT groups, real fractions and reset times.
  Both chat messages and submitted bridge-terminal lines use this read-only
  command without a model turn. The bridge terminal is not agy's native TUI.
  A raw-PTY regression fails without the terminal input reader.
  Legacy/unowned MCP entries are preserved; a separate `sandhill-managed`
  entry is tracked for safe updates/removal when the original name is taken.

- **RStudio/Positron borrowing** (2026-11-11, Huan's approved plan):
  docked find/replace (⌘F, ⌥⌘F, ⌘G/⇧⌘G) across scripts and all notebook
  cell sources, with one undoable Replace All; folding for `# Section ----`,
  Markdown headings, R braces and Python blocks; an Outline popup in the
  editor row; R-only ⌥- (` <- `) and ⇧⌘M (` |> `, or ` + ` in ggplot).
  Completion merges live kernel names with ty/Ark LSP items; ruff/Ark
  diagnostics map back to cells. Object Explorer tabs, copy-access-code and
  bounded object search; Data Explorer column hiding with projected reads and
  import Copy/Insert; fixed-size plots; ⌘-click project paths in the terminal;
  Quarto Render jobs with output actions; kernel reason labels; pinned
  Welcome projects; Focus Editor (⌃⌘↩); agents see Data Explorer state and can
  read Viewer pages; notebook Explain/Improve start new conversations.
  Verified in an isolated `cargo run` with real Ark, ipykernel, Seurat, ty and
  ruff (Sandhill-owned copies installed into the isolated home), light/dark
  and normal/minimum widths. Not verified: an agent actually answering
  Explain/Improve (the isolated home had no agent sign-in) and remote SSH use.

- **Huan's 2026-11-11 feedback, small items**: the editor kernel picker's dot
  follows the kernel name. Console input always uses the session/document language, and past
  Console code is highlighted with cached tree-sitter text in the editor theme.
  Python `in`, `not in`, `is`, `is not`, `and`, `or`, `not` use the keyword
  colour, as in Xcode. The status bar shows one chip per AI provider; a chip
  opens only that provider's quota and resets, with "All AI accounts…" for
  the full list. Claude and Codex logos are transparent and Codex is blue.
  Files rows show a ring when the file is open in a tab, the kernel's dot when
  it has one (green running), and a grey dot after the name when unsaved.
  Verified in an isolated `cargo run` with a live ipykernel, light and dark.

- **Huan's 2026-11-11 feedback, approved items**: Console plots stay inline
  (hover "Send to Plots", header "Send all plots to Plots", opt-in automatic
  sending per device; sent plots are remembered). The Console's own picker
  shows the bound file with its kernel, lists the device's sessions and new
  kernels, follows the active or running document and Kernels-pane clicks,
  and warns once when kernels exceed 80% of device memory. Codex and
  Antigravity command-line mode runs their native TUIs on the same
  conversation (`codex resume --no-daemon`, `agy --conversation`), handing off
  exclusively and importing the turns back into Chat. `.qmd`/`.Rmd` get add
  rows and A/B with byte-preserving insertion after YAML. Welcome shows one
  tab per device. The busy cursor is gone: documents open at once with a thin
  progress bar and lazily built cell views (1,500-cell notebook first draw
  102 ms → 8 ms). Verified natively in an isolated `cargo run` (Console plots
  and picker, send-all, qmd insert and save, loading bar, Welcome tab); native
  agent TUIs were verified by daemon tests with fake CLIs only.

- **OpenScience in the AI pane** (2026-11-11, Huan's approved plan): a new
  chat-only agent through the generic ACP bridge (`sandhilld acp-chat`), with
  its models in the model popup and the Synthetic Sciences mark. Its built-in
  `python`/`r` tools are denied per process, and Sandhill's MCP tools are
  served to it over loopback HTTP with a per-conversation token (OpenScience
  ignores ACP-supplied MCP servers and sandboxes local ones without sockets).
  Verified with the real openscience 2.0.153 and a ChatGPT (Codex) model: it
  ran code in the live Python session after a chat approval card, the Console
  showed "run by OpenScience" and Variables updated.

- **Huan's 2026-11-11 feedback, second batch**: Welcome's device tabs head
  one panel with the project list (status dot after each name, no
  "connected"); Open Folder…, Connect to Server… and Resources and storage…
  sit above it, and the list scrolls inside a fixed-height box. Quota rings
  carry `5h`/`7d` inside; a provider's popup has a larger bold name and icon,
  then a small accent switch and Refresh below the windows. Claude has no
  read-only quota command, so its popup says quota appears after a Sandhill
  Claude response. The plot zoom settles (one-line status, ±1px fit
  tolerance). `.qmd`/`.Rmd` gain a Notebook/Source toggle and byte-preserving
  cell delete (button or D D, Z restores). Language analysis drops stale
  diagnostics and hovers, keeps each cell's own language, and shows one
  bubble at a time. Diagnostic bubbles and switch thumbs get solid colours,
  since Kit draws them over the transparent glass `background`.

- **Huan's 2026-11-11 feedback, third batch** (plan in
  `docs/design/done/ui-feedback-2026-11-11-batch3-plan.md`): Welcome's device tabs are
  larger, Connect to Server… ends the tab bar, Open Folder… ends the Projects
  row, Resources and storage… sits under the panel, and the first two open
  centred dialogs (`folder_dialog.rs`, shared with the plot window). Project
  names in the title bar are bold. Pane tab bars and tool rows float at 82%
  opacity over their scrolling content (`look::floating_header`,
  `look::PaneScroll`). The model popup has no effort text beside its title,
  its slider snaps to ticks, and popups no longer pass scrolling to the
  conversation. Chat/command-line switching fails visibly on bridges too old
  to report the mode. Notebook cells hide line numbers; add-cell buttons are
  centred. New `.qmd` files no longer start with an `{r}` chunk. Image
  outputs gain a magnifier; the plot window opens at the picture's size,
  moves by its title, has Size follows window and Redraw switches,
  px/in/cm units with an editable dpi, and a remembered folder (default
  `plots/`, created on save).

- **Huan's 2026-11-11 feedback, fourth batch**: Welcome's three actions share
  one outline style, project names align with the first device tab's title
  and statuses with the buttons' text. `@`/`#` lists follow Up/Down/Enter/Tab.
  History's Delete? is white on red. Mode switching restarts an old bridge on
  a matching daemon, otherwise explains the outdated service; AI banners close.
  The Data Explorer has one 29px bar (stats toggle, label, refresh, Copy as
  code, chips, Add filter toggle, search, Export icon), full type headers,
  zebra rows, selection size bottom-left and shape bottom-right.
  Explorer and notebook tables (`output_table.rs`, `table_selection.rs`)
  select by drag, rows, columns or ⌘A and copy TSV without headers.

- **Welcome as a settings-style window** (2026-11-11, plan in
  `docs/design/done/welcome-sidebar-plan.md`): a glass sidebar of Devices (status
  dots, right-click Connect/Disconnect/Remove), "+ Connect to Server…",
  Manage → Resources and Storage and the crane; the content shows the device
  with a left-aligned title, Connect/Disconnect, a per-server "Connect
  automatically" switch (`serverAutoConnect` in `gpui.json`), the grouped
  project list and Open Folder…. The window's minimum size is 720×480.

- **Oh My Pi and pi in the AI pane** (2026-11-12, plan in
  `docs/design/done/pi-omp-plan.md`): two more agents in the provider picker,
  each with chat and command-line modes, its models and thinking levels in
  the model popup (listed without a model turn), resume, approval cards,
  Sandhill's tools, its official mark, and no quota chip. Oh My Pi (`omp`)
  goes through the generic ACP bridge (`acp-chat --agent omp`); pi through
  its own `pi-chat` bridge over `pi --mode rpc`. The provider list sorts
  names case-insensitively, so pi sits before Posit Assistant.

- **Huan's 2026-11-12 feedback**: the title bar's `+` shows Welcome for the
  current device without opening the folder dialog. The model popup shows
  the thinking effort for whichever agent's tab is open (Favourites: the
  box's agent); in a new conversation, a level set on another agent's tab
  goes with a model then picked there. Thinking effort is one segmented bar
  (`effort_bar.rs`, after Huan's Dribbble reference): the fill runs to the
  chosen level with its name, ticks divide the rest, clicks and drags snap,
  the fill springs and glows, and the strongest level is warm. Agent marks
  have no background: OpenAI's logo (Codex) and Synthetic Sciences' mark
  (OpenScience) follow the text colour; Posit Assistant uses Posit's mark and
  Oh My Pi its gradient π. Streamed reasoning updates one step row instead
  of adding one per update.

- **Welcome lists projects** (2026-11-12, plan in
  `docs/design/done/welcome-projects-plan.md`): Welcome draws no capsules; its
  sidebar and content reach the window top. Above Devices, Open lists this
  window's projects (click to switch, right-click to close) and Running the
  not-open projects with live kernels on connected devices (click to open),
  noting servers that aren't connected.

- **Trusting a folder** (2026-11-12, plan in
  `docs/design/done/folder-trust-plan.md`; `trust.rs`): the first open of a folder
  asks "Do you trust the files in …?". Trust Folder is remembered on that
  device, with the folders inside; Open in Restricted Mode shows files, saved
  outputs and previews, starts nothing (kernels, Terminal, Jobs, Git, code
  checking, AI), and puts a "Restricted" chip in the status bar that asks
  again. The daemon enforces it; projects opened before are trusted. Welcome
  → Manage → Trusted Folders lists and removes them. Codex and pi are told
  per run and do not ask again; Claude Code, Posit Assistant and Antigravity
  keep their own question.

- **Welcome's Projects and Resources pages** (2026-11-12, plan in
  `docs/design/done/welcome-projects-resources-plan.md`): the sidebar is
  Projects, Devices and Manage, one row selected, its page on the right.
  Welcome opens on Projects: Pinned (the star on any device's or this
  page's rows), Open and Running, each project once with its status.
  Resources and Storage is a page (also from a project window's device
  menu): device ticks and Read, a card per ticked device with what runs,
  storage grouped by category with a size bar and tick boxes, the projects
  folded into one line, then the actions each beside what it does. Its state
  is dropped when another page is shown, unless an action runs.

- **Idle agents, and Welcome as its own window** (2026-11-12, plan in
  `docs/design/done/idle-agents-welcome-window-plan.md`): the daemon stops a
  resumable conversation idle for 30 minutes (`exit.idle`), and the next
  message resumes it; History rows of running conversations have a stop
  button. Welcome is its own window, as wide as its sidebar and widest page
  (`start_screen::WELCOME_WIDTH`) and half the display's height, at least
  800×600 and never resized smaller (`app_window::show_welcome`), and
  there is one maximized main window: projects chosen in Welcome open there
  and Welcome closes; `+` brings Welcome back; closing the main window, or
  its last project, shows Welcome; closing Welcome alone quits. The sidebar
  is Devices, Projects, Manage, opening on This Mac's page; devices can be
  renamed (double-click or right-click) and projects given a built-in icon
  (right-click → Change Icon…); rows count running agent conversations.

- **Streamed replies fade in** (Huan, 2026-11-13): agent replies use Kit's
  `TextView::stream_fade`, so the words a chunk adds light up one after
  another (off with reduced motion), and each code block has a copy button
  in its corner (`agents_view::code_copy_button`).

- **Dependencies brought up to date** (Huan, 2026-11-13): Rust 1.100; Kit,
  Base, Component and WebView 0.9.1 with Fast 0.1.15 and Pre 0.3.12 (no code
  changes needed); in the daemon jupyter-zmq-client 3.0.1 with zeromq 0.7,
  rusqlite 0.42, scraper 0.29, sha2 0.11, base64 0.23 and tokio 1.54; here
  resvg 0.50 and base64 0.23; the bundled ty 0.0.102 and Ruff 0.17.5. sha2
  0.11 no longer formats a hash as hex or takes `io::copy`, so checksums go
  through `sandhill_proto::hex`. `tree-sitter` stays at 0.26, the version
  Kit 0.9.1 needs, as Cargo allows only one copy of it. GPUI Fast still
  uses resvg 0.46 and GPUI Pre 0.45, so Sandhill's own 0.50 is a third copy.

- **Text files, Variables, Kernels rows and the quit card** (Huan,
  2026-11-13): `Format::Shell` became `Format::Text`, which also opens JSON,
  YAML, TOML, `.txt` and `.gitignore` (shell colours for its comments), read
  within the 5,000,000-byte text limit; Kit's YAML and TOML grammars are on.
  Variables, Help and Packages follow the Console's kernel
  (`variables_session`), so a file without a kernel no longer empties them,
  and a list from a kernel that has gone is not drawn. A Kernels row is one
  line, "Python 3.12 (omicverse) · qc.ipynb". The quit question is the
  smaller card described in `CLAUDE.md`.

- **Agent accounts** (2026-11-13): the quota panel lists accounts per device,
  with explicit conversation switching and last-known usage. Welcome → Manage →
  Agent Accounts adds, renames, removes and signs in through the agent's own
  embedded terminal. New conversations start with the account set by the
  quota panel's "Use for new chats" (switching a conversation doesn't change
  it; Huan, 2026-11-13); a limit banner offers alternatives without
  switching automatically. Antigravity API-key accounts bill the key rather
  than the subscription.

- **Remaining quota and banked resets** (2026-11-13): provider popups and
  All AI accounts share compact account cards with remaining-capacity bars,
  reset countdowns and clear last-known readings. Status bar rings also show
  quota left, using green, amber and red capacity thresholds. Codex can use
  its soonest-expiring banked reset after an inline confirmation; the returned
  account immediately updates its card and chip. Claude links to its usage
  settings for limit resets.

- **Wrapping, maximizing, hiding the middle, account defaults** (Huan,
  2026-11-13): the document action row has a Wrap toggle, saved per editor
  type as `editorSoftWrap` (`markdown` and `text` wrap by default; `r`,
  `python`, `bash`, `json`, `yaml`, `toml` and other code do not); a notebook
  follows its selected cell's type, and CSV/TSV and the Console stay
  unwrapped. Each pane's tab bar has a maximize button (Escape restores);
  the title bar gains a middle-column toggle and a house button for Welcome.
  With no switchable conversation shown, quota cards offer "Use for new
  chats". Claude's folder-trust question sits above the composer. It never
  showed while the chat was in front because an undrawn terminal got no new
  frames (frames were credited only after a paint); `TerminalView` now
  credits an undrawn terminal's next frame itself, every 100ms at most.

- **Markdown images and every catalog file type** (Huan, 2026-11-13; plans
  `docs/design/done/markdown-images-plan.md`, `file-opening-plan.md`):
  `md_images.rs` rewrites a rendered copy of the Markdown so Kit only gets
  owned images (daemon `ReadBytes` for project files, `ureq` for `https://`,
  notebook attachments, `data:`), with placeholders giving the reason.
  `bio_text.rs` decorates FASTA/FASTQ, tab-separated bio formats and
  Nextflow/WDL/BibTeX keywords (first 256 KB); unknown files are sniffed
  from an 8 KB prefix; text over 5 MB opens read-only (`Notebook::truncated`,
  no save or replace). Checked in the app: PNG, SVG, missing and outside
  images and a table; FASTA, FASTQ, VCF, Nextflow, Snakefile, a 6 MB VCF, an
  unknown text file, the binary question and both right-click menus.

- **AI pane header and code colours** (Huan, 2026-11-13): the conversation
  name is plain text (double-click still renames); Kit's `tree-sitter-sql`,
  `-diff`, `-html`, `-css` and `-javascript` colour those code blocks in AI
  replies (checked with Kit's highlighter in a throwaway test).

- **Pane buttons and Help results** (Huan, 2026-11-13): pane buttons use
  Lucide fold-vertical/unfold-vertical (collapse, fill the column; restoring
  uses the opposite) and maximize/minimize corners (to and from the window).
  Opening a Help topic from search results, by click or Enter, folds the
  results to a "Show N search results" row so the page starts near the top;
  typing a new search shows them in full.

- **Completion, signature hints and Console history** (Huan, 2026-11-13;
  `docs/design/done/completion-console-plan.md`): `language/providers.rs` filters
  and ranks every answer itself (prefix, any-case prefix, word starts,
  subsequence; call arguments and `library(` packages first) and asks from
  the first letter and after `$ @ . [ :: ( ,`; Tab opens the list;
  `language/signature.rs` draws the signature hint (`signatureHelp`, else the
  kernel's new `inspect`). Python shows `ty` at once with 300 ms grace for the
  kernel. The Ark Console completes through Ark's language server on a
  one-cell document. Up/Down walk the session's commands from an empty
  Console input (`console::history_step`). Kit's caret menus open below the
  caret only, so `vendor/gpui-component` flips them above when needed.
  Checked with real Ark and Python: `libr`, `rnorm(`, `np.`, `np.mean(`, the
  Console history and the Ark Console list opening upward.

- **Completion timing, snippets, hints, brackets, drafts** (Huan,
  2026-11-13): `providers.rs` turns snippets into text and moves the cursor
  to `$0` after Kit inserts the item (Kit has no accept hook), adds `()` to
  Python functions, and delays a word's first two letters by 3 s.
  `signature.rs::hint_top` keeps the hint above the caret line, below the
  title bar, and hides it while Kit's menu may open above. `rainbow.rs`
  colours brackets by depth (terminal yellow, magenta, blue; under 256 KB);
  `register_r_editing` gives R explicit auto-close pairs (Kit's auto-close
  doesn't run for text committed by an input method). `console.rs` keeps a
  draft per session. The Files toolbar row and its project menu (whose
  items remain in the ☰ menu) are gone. Checked with real Ark and Python.

- **Choosing folders** (Huan, 2026-11-14; `folder_dialog.rs`): on this Mac,
  `choose_local_folder` shows the system's folder window for Open Folder
  and the plot folder; Welcome's content takes folders dropped from Finder
  (`ExternalPaths`), opening each on this Mac; the daemon refuses files.
  Servers keep the built-in chooser: places (Home, `/`, the folders holding
  recent projects or the project), clickable path, one field that filters
  or takes `/`/`~` paths, ↑/↓, Return, ⌘↑/⌘↓, ⌘Return, ⌥⌘N, a click enters
  and a double click opens, faded files, a dot-name toggle, and Git/Sandhill
  marks from `ListDir { marks }`. Kit's Input binds Return, ⌘↑ and ⌘↓, so
  the card captures those actions rather than keys. Checked in the app with
  the dialog forced locally, a simulated Finder drop and the system window
  opening; that window's contents could not be captured.

- **Installing IRkernel** (Huan, 2026-11-14; `irkernel.rs`, the daemon's
  `irkernel.rs`): `list_r_without_irkernel` is read with the kernels. The
  picker's install rows sit below a divider and project settings' R row
  lists them. The confirmation shows the daemon's `plan`, and
  `install_irkernel` starts a job tracked until it ends (event or list
  refresh), when the kernels and offers are read again. Checked by
  installing for Homebrew R 4.6.2 and running code in the new kernel.

- **Pickers, notebooks, explorer, completion docs** (Huan, 2026-11-14):
  - `kernel_menu.rs` hides the environment line a kernel's name already
    contains.
  - `view.rs::box_takes` no longer assumes an output box adds every wheel
    turn: gpui-fast skips turns that can't move it, which left the notebook
    unable to scroll under outputs.
  - `cell_view.rs::run_line` shows `Cell #N`, `Output [n]` and the time on
    every code cell.
  - `explorer.rs` resizes columns from their header's right edge.
  - The vendored completion menu (see its `SANDHILL.md`) has kind badges,
    places the list by its own size, and keeps a fixed, resizable docs
    panel beside it inside `set_popup_insets`.
  - `doc_text.rs` turns plain docstrings into markdown (ty's completion docs,
    the Help pane) and shows R help `<pre>` blocks as R code.
  - Checked in the app with real Python and Ark.

- **Welcome project actions, notebook margins, data explorer** (Huan,
  2026-11-14; plan in `docs/design/done/welcome-actions-explorer-plan.md`):
  - Welcome rows get a ⋯ menu (`start_screen.rs::render_project_menu`).
    `app_window.rs::ask_close_tab_then` asks once about unsaved files and
    running kernels, then runs an optional follow-up (the list removal),
    reaching the main window from Welcome through `with_main` /
    `with_welcome`.
  - Notebook margins: `cell_view.rs::GUTTER` is 18px, so with the list's
    6px padding both sides are 24px.
  - `explorer.rs`:
    - `ExplorerWindow` hosts an explorer moved out by `PopOut`. The
      workbench keeps it in `popped_explorers`, so runs still refresh it
      and closing the project closes it.
    - Pinned columns come first in `visible_columns` and are drawn in an
      overlay offset by the sideways scroll.
    - The ⋮ and gear menus are `render_column_menu` /
      `render_settings_menu`; `table.rs::sorted_by` replaces click cycling.
  - New Lucide icons: `more.vertical`, `window.pop-out`, `column.select`,
    `sort.clear`, `pin.off`.
  - Checked in the app with real Python.
- **Light-theme icons, Sandhill's file icons, project icon** (Huan,
  2026-11-14): `look::picture` loads `<icon>.light.svg` while the light
  theme is on (`LIGHT_ICONS`, set in `set_appearance`); `build.py` makes
  them (see the icons README). 14 formats Gruvbox lacks have Sandhill's own
  drawings, and PDF, SVG and agent notes Gruvbox's own artwork.
  `look::project_icon` defaults to `folders/project`. Checked in the app in
  both themes.
- **Round tabs and capsules** (Huan, 2026-11-14), in `look.rs`:
  - `capsule` is a pill with the `cap_bg` wash and no edge; `tab_capsule`
    is its 24px track (`TAB_TRACK`), and `View::tab` a 20px pill (`TAB_H`)
    in the `TAB_GROUP` hover group. `raised` is `cap_on` on a soft shadow.
  - `tab_close` / `close_circle` draw the 16px close button; `place_close`
    puts it after the label for the picked (or unsaved) tab, otherwise over
    the icon via `tab_lead`. GPUI applies hover styles only when painting,
    so a hover-only button cannot take room.
  - The project capsule is no longer raised; its name colour marks it.
  - Checked in the app in both themes, including closing an unpicked tab
    from its hover button.
  - Closing a tab that isn't in view keeps the view
    (`workbench.rs::close_tab` brings back the document shown before and
    whatever covered it); closing the picked tab still shows its neighbour.
- **One palette for icons and the crane** (Huan, 2026-11-14): no app code
  changed. `docs/design/ui/icons/build.py` repaints Gruvbox's and
  Sandhill's file/folder icons in Xcode's colours (see the icons README),
  and `docs/design/ui/brand/build.py` gives the crane a teal feather, the
  same red and a neutral white; `assets/icon/icon.icns` and
  `assets/brand/icon.png` were remade from it.
- **Explorer keeps pins and widths; more bioinformatics text** (Huan,
  2026-11-14):
  - `explorer.rs::SavedView` also holds the pinned columns and the widths
    the user dragged (`Explorer::resized`), by column name; saved on pin and
    when a drag ends, restored in `took_shape`.
  - `bio_text.rs`: `tree_marks` for Newick and Nexus (branch lengths, quoted
    names, nested comments; Nexus words only in a `#NEXUS` file), and UCSC
    `track`/`fixedStep` lines as headers in tables. `notebook.rs` maps the
    new extensions (PAF, PSL, gVCF, BEDPE, peaks, wiggle, `.fai`, trees…);
    `.tab` opens as tab-separated; indexes and bigWig count as binary.
- **More files split** (see `docs/design/done/file-split-plan.md`): `view.rs`,
  `plots.rs`, `git_pane.rs` and the protocol's `lib.rs`.
- **Zarr folders and PLINK files** (Huan, 2026-11-14): `look::folder_icon`
  gives a `.zarr` folder its format's icon; `look::file_icon_beside` tells
  PLINK's `.bed` from BED intervals in the Files pane.
  `documents.rs::opens_as_text` reads a `.bed`'s first bytes before opening
  it (`notebook::shared_with_binary`), so genotypes are offered to the
  default app instead of failing as text.
- **Third-party notices in the bundle** (Huan, 2026-11-14): the Mac
  workflow runs `notices/make.sh` (cargo-about, installed by
  `taiki-e/install-action`) and copies `sandhill-app.txt`, `sandhilld.txt`,
  `notices/libghostty-vt/*`, Sandhill's `LICENSE` and the code font's
  `OFL.txt` into `Resources/licenses`; Ark's `LICENSE` and `NOTICE` go
  beside it as `daemons/ark-<triple>.notices`.
- **New conversation screen** (Huan, 2026-11-14; plan in
  `docs/design/done/new-conversation-plan.md`): `agents_start.rs` replaces
  `render_new`. `Agents::start` (`StartContext`) holds the project's live
  sessions, the open document and a table in a session, read by
  `refresh_start` when the screen shows (again after 15 s). A starter fills
  the message box and its chips through `add_ref`; nothing is sent.
  Checked in the app in light and dark, including filling the box; the
  Continue rows were not seen, as the test project had no conversations.
- **Status bar quota and Welcome build id** (Huan, 2026-11-14): Posit
  Assistant joins OpenScience, omp and pi in `quota_providers`' exclusions.
  With it went `QuotaMonitor`'s own quota query (`Quotas`, `ingest`,
  `refresh_provider`), which only Posit still used; every remaining
  provider is read through `AccountManager`. The Welcome sidebar shows the
  first seven characters of `SANDHILL_COMMIT`.
- **No automatic quota checks** (Huan, 2026-11-14, after Google suspended
  an Antigravity account): `QuotaMonitor` lost its 60-second poll, and
  opening the panel no longer asks the agents. `agent_accounts::Ask` says
  whom a listing asks: `Nobody` (the saved answers) or `Accounts(ids)` (a
  provider's Refresh button, Welcome's Refresh for every account shown, or
  the account a sign-in was for); `ListAgentAccounts` gained `ids`. Saved
  measurements no longer expire after five minutes, only when their
  window resets; each provider's Refresh button says how old they are
  (`checked_label`, from its accounts' newest `observed_at`).
- **Antigravity turned off; Codex by default** (Huan, 2026-11-14):
  `agents::turned_off` hides an agent from the model rail, favourites,
  quota chips and Welcome's accounts, skips its model listing, and `say`
  refuses its messages, old conversations included. `default_kind()` is
  Codex, which new conversations and cell suggestions start with.
- **Pane height icons** (Huan, 2026-11-14): Sandhill draws them in Lucide's
  stroke style (`vendor/sandhill/icons/pane-*.svg`): a short window
  collapses, a tall one fills the column, two overlapping windows restore
  the split. They replace Lucide's fold/unfold and `rows-2`.
- **SVG pictures** (Huan, 2026-11-14): resvg was built without
  `raster-images`, so the PNGs that matplotlib and scanpy embed in an SVG
  (each spatial plot) were skipped and only axes and text showed. The
  feature is on; outside files and URLs are still never read.
- **Opening files polish** (Huan, 2026-11-14;
  `docs/design/done/open-files-polish-plan.md`):
  - The delimited dialog (`delimited.rs::render_delimited_dialog`) has a
    `Choice` (Text or Import, `first_choice`), sizes from `git::file_size`.
  - `colouring.rs`: the action row's colouring menu. `notebook::COLOURINGS`
    lists the choices; `Notebook::recoloured` sets the format (comma/tab →
    `Delimited`) and language; `show_notebook` applies
    `prefs::text_colouring`, and saving keeps it. A delimited document's
    separator now comes from its language (`delimited::separator`), not its
    file name. `notebook::looks_tab_separated` opens a tab table saved as
    `.txt` as `Delimited`.
  - Office files are binary (`notebook::is_binary`) with Word, Excel
    (Sandhill-drawn) and PowerPoint icons.
  - Previews: `Actual Size` replaces `100%`; `media/native.rs::HandView`, a
    transparent overlay added only while the hand is on, drags the PDF or
    image scroll view and sets open/closed hand cursors. Dragging was not
    tested in the running app: injected mouse events do not reach it.
- **Trash, counts, names** (Huan, 2026-11-14):
  - Move to Trash on macOS goes through `NSFileManager trashItemAtURL`
    (`fileops.rs::mac_trash`): each disk's own Trash, so files on flash
    drives no longer fail as "on another disk", and Finder can Put Back.
    Checked with a temporary exFAT disk image.
  - The action row starts with `document_size`: "ln", CSV/TSV "col" (from
    the pinned header, `delimited::column_count`), and a selection's lines
    or, on one line, its columns (`delimited::fields_spanned`) or
    characters. `show_notebook` observes the editor and redraws only when
    the selection or length changes.
  - The run line reads `#3`, without "Cell".
  - The daemon lists every Ark kernel as "Ark R" (`kernelspecs::shown_name`).
- **Plot window** (Huan, 2026-11-14; `docs/design/done/plot-save-window-plan.md`):
  `plots/zoom_view.rs` lays the preview out full height with a 260px
  settings column (under the preview below 720px), toggled by a title-row
  button (`Zoom::settings`, `prefs::plot_settings_shown`). PDF has a
  Rasterize switch (`Zoom::rasterize`, `prefs::plot_rasterize`) sent as
  `RedrawPlot::rasterize`; the daemon (`redraw.rs`) rasterizes matplotlib's
  data artists, and R's whole plot through a base-R BMP read back into
  `grid.raster`. Checked in the app: a saved PDF holds an image and fonts.
- **Data explorer redesign** (Huan, 2026-11-14;
  `docs/design/done/data-explorer-redesign-plan.md`): `Explorer::render_top`
  is the action row (name, status dot, Search, Filter, Export menu, icons);
  `render_filter_row` puts filter and sort chips over the table only while
  some are on; `render_side` is the column panel (heading, `column_find`,
  one line per column with `type_badge` and the checkbox last) and
  `render_stats` its folded statistics. Menus open under their buttons
  (`menu_under`, through `popup_triggers`). `fit_column_widths` measures a
  new table's first rows with the window's text system on the next render
  (24 longest values per column, so narrow digits don't hide wide ones).
  Category colours no longer go missing when the summary arrives before
  the first rows (`summary_generation`). Checked in the app in light and
  dark; typing into Find column could not be driven by the test tool.
- **Compress and extract** (Huan, 2026-11-14): the Files menu's Compress to
  .zip, .gz/.tar.gz and .gz (bgzip), Extract Here and Decompress Here
  (`archive_tasks.rs::archive`) send `Call::CompressPath`/`ExtractPath`; the
  daemon's `archives.rs` works beside the original under free names
  (claimed with `create_new`), with zip, flate2, tar and noodles-bgzf in
  pure Rust (zlib-rs) so server builds need no C libraries. bgzip runs on
  up to 8 threads. Zips and tarballs unpack into a hidden staging folder,
  then their one item or the folder moves out; a failure removes what was
  half made. Links stay links; zip64 is used for files from 4 GiB.
- **Archive progress** (Huan, 2026-11-15): calls given a `task` id count
  the input bytes read (`archives::Counted`) against the input's size;
  `Call::ArchiveProgress` reads it, `Call::CancelArchive` makes the next
  read fail with `CANCELLED_ERROR` and the half-made archive is removed.
  The app asks four times a second (`archive_tasks.rs`) and draws the bar,
  percentage and Cancel in the status bar; a cancelled task reads
  "Stopped …", not an error. Checked on a 600 MB file.
- **Closing a file** (Huan, 2026-11-15; `docs/design/done/close-file-plan.md`):
  `ask_close` asks `close_ask` for the file's live kernel (with its other
  users from `used_by`) and the running conversations `close_file::mentions`
  ties to it (pin, starting refs, the report's `document`, or a prompt's
  `place.refs`). Any found bring up `render_close_card`; ticked items are
  shut down (`shut_down`) or stopped (`StopTerminal`, which keeps them
  resumable) as the tab closes. Checked with a live kernel and a Codex
  conversation @-mentioning the file.
- **Documentation text** (Huan, 2026-11-15): hover, completion and Help
  documentation show prose in the interface font and code a pixel smaller
  in the code font (the vendored popovers' `DOC_TEXT`/`DOC_CODE`,
  `help.rs::help_style`); the hover popup's corner drags to resize it.
  Completion items without documentation get it when selected:
  `providers.rs` registers `Provider::docs` with `set_completion_docs`;
  Ark's items go back to it as `completionItem/resolve`, Python kernel
  items carry their dotted name (`sandhill_help`) for `Call::HelpPage`.
  Answers are kept per editor and filled in on the next answer. Kernel
  items no longer say "Live kernel". Checked with Ark R (`read.cs`, hover
  on `read.csv`) and the Python Console (`sc.Ann`).
- **Documentation delay, kernel names, F1** (Huan, 2026-11-15): the
  completion menu's documentation shows 3 s after the last keystroke, or at
  once on Tab (`complete_on_tab` calls `show_completion_docs_now`). R
  sessions are labelled by kernel (`machine::r_kernel`: Ark, or IRkernel
  for `ir`, `ir-…` and conda's `…-r`) everywhere `session_label` is used.
  F1 (`help_at_cursor`) now also works in the Console, takes the language
  from the focused editor, moves the Console to the file's live session
  first, and says so in Help when no session of that language runs.
- **Red button labels in dark mode** (Huan, 2026-11-15): Kit's dark theme
  labels its red (red-400) buttons with red-600, which can't be read.
  `look::brand_colors` sets `danger_foreground` and
  `button_danger_foreground` to `#450a0a` in dark mode (about 6:1), as
  navy labels the light blue. Kernels' Shut down, Stop All and Quit, and
  Kit's red tags use it.
- **Kernels whose program is gone** (Huan, 2026-11-15): a Homebrew R
  upgrade left an IRkernel `kernel.json` naming
  `/opt/homebrew/Cellar/r/4.4.2_2/lib/R/bin/R`, listed as "New R" and
  failing to start. The daemon's `kernelspecs::list` now drops kernels
  whose program is missing (`runnable`: a path must be a file, a bare name
  must be on the kernel's `PATH`); `find` still resolves them, so a
  document that chose one gets the clear "wasn't found" error.
- **General settings** (Huan, 2026-11-15): Welcome's Manage → General
  (`welcome/start_screen/general.rs`) shows this Mac's choices for every
  project, read in one go by `prefs::general`. ⌘, (`OpenSettings`), the
  Sandhill menu's Settings… and the palette open it. Choices open projects
  must follow (notebook runs and plots, Suggest, following the AI, Wrap,
  the Files sort) go through `shell::choices::choose`, which saves once and
  applies to every project in the main window (`app_window::update_projects`);
  the ☰ menu, Suggest, the AI pane's follow switch, Wrap and the Files sort
  menu use it too, so they now change all open projects, not only the shown
  one. The theme gained System (`look::choose_appearance(None)`), which the
  title bar's button alone could not return to.
- **Project settings redrawn** (Huan, 2026-11-15;
  `docs/design/done/project-settings-plan.md`): the card uses General's
  groups and rows, now shared in `look.rs` (`settings_group`,
  `settings_row`, `settings_label`, `settings_detail`). R and Python use
  the Console's round picker with its list floating in
  `composer_view::popup` (it used to open inside the card and push rows
  down); where documents run and where chats are kept are capsule tabs;
  the R and Python notes are one grey line each; IRkernel offers sit under
  the R row. What is saved is unchanged.


## Running it

```sh
cargo build -p sandhill-daemon        # at the repo root, once
cd app
cargo run -- /path/to/project         # the folder is optional
```

For a local test **app bundle** that connects to Linux servers, also build its
matching server daemons before signing it:

```sh
# From the repository root. Zig must be on PATH.
cargo install cargo-zigbuild --locked       # once
rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl  # once
bash app/build-server-daemons.sh
```

The default output is the debug app's `Contents/Resources/daemons`; an optional
first argument selects another output directory, as the Mac build workflow does.
This builds/copies files only, without connecting to servers. The packaged app
uses bundled payloads on its next SSH connection. For `cargo run`, build to
`app/target/debug/daemons` instead:

```sh
bash app/build-server-daemons.sh app/target/debug/daemons
```

The development Cargo config selects that directory and the root workspace's
debug `sandhilld`; explicit environment overrides still win. Rebuild the daemon
and matching Linux payloads after changing daemon code.

On Linux GPUI also needs `libxkbcommon-dev libxkbcommon-x11-dev`,
`libwebkit2gtk-4.1-dev` for the web view, a Vulkan driver
(`mesa-vulkan-drivers` works, even without a GPU) and X11 (not Wayland).

`cargo test` here includes a test that runs a cell in a real Python kernel; it
skips when sandhilld isn't built or no Python kernel is installed.
