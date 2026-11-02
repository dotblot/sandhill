# Sandhill's Mac app

Sandhill.app, drawn with [GPUI](https://gpui.rs), Zed's UI toolkit. It
started as a trial (milestones 1 to 4 below); on 2026-11-01 it was chosen to
replace the first app, built with Tauri in `app/`, and on 2026-11-02, once it
did everything that app did, the Tauri app was deleted (it is in the git
history, last in commit d3ff9f6). It keeps the Tauri app's look, with fonts
2px larger. "The Tauri app" in comments here means that app.

It reaches `sandhilld` through
`sandhill-link` and `sandhill-proto`, and it borrows the daemon's own rules for
building outputs and finding Quarto chunks (`sandhill-daemon`'s `outputs` and
`qmd`). It is a separate Cargo project (its own `[workspace]`), so its many
dependencies stay out of the main build, its `Cargo.lock` and CI. UI pieces
come from `gpui-kit` (Apache-2.0); nothing is taken from Zed's GPL editor.

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
  runs in the same kernel (Shift+Enter). With no notebook open, **Start**
  gives the Console a kernel of its own; **Stop** shuts it down.
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

- `src/look.rs` holds the Tauri app's colour tokens (its `styles.css`),
  light and dark, and builds gpui-kit's theme from them, so they are
  written once. It follows the system's appearance; `SANDHILL_APPEARANCE=dark`
  (or `light`) picks one, for trying it.
- Every text size is the Tauri app's plus 2px: window text 15px, code 15px
  in Maple Mono NF, small rows 13px. Maple Mono is bundled as gzipped TTF in
  `assets/fonts` (GPUI reads TrueType, not the web fonts); `make.py` there
  makes them from the WOFF2 web fonts.
- Icons come from `docs/design/ui/icons` (drawn by its `build.py`), put
  into the binary and served as `sandhill/…`; file and folder icons are
  picked by the rules in its `registry.json`, as in the Tauri app.
- The window is glass panes 3px apart on the warm wall under a 31px title
  bar of capsules (on a Mac the title bar is Sandhill's own, with the
  window buttons at its left), the files pane directly on the wall, a
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

## Step 2: Agents

The AI pane, as the Tauri app has it (`AgentsPane.tsx`, `AgentChat.tsx`,
`AgentComposer.tsx`, `agentEdits.ts`), text 2px larger.

- **2a** (`agents.rs`, `agents_view.rs`, `chat.rs`): conversations are the
  project's, not a file's. The chat shows your messages, the agent's
  replies, its steps (read, edit with a diff, bash) and what it asks; the
  terminal button shows the agent's own terminal. History lists past
  conversations by day; New picks Claude Code, Codex or Antigravity.
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

- **7a** (`app_window.rs`, `start_screen.rs`): a window holds a tab per
  machine in the title bar, each its own connection with one project open;
  a project keeps running while another tab is shown. Opening a project on
  a machine that has a tab replaces the project there. Right-click closes
  a tab, after asking. The windows and their tabs open again at the next
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
  shows its details. The status bar's connection opens a pop-up with what the
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

## Running it

```sh
cargo build -p sandhill-daemon        # at the repo root, once
cd gpui
cargo run -- /path/to/project         # the folder is optional
```

On Linux GPUI also needs `libxkbcommon-dev libxkbcommon-x11-dev`,
`libwebkit2gtk-4.1-dev` for the web view, a Vulkan driver
(`mesa-vulkan-drivers` works, even without a GPU) and X11 (not Wayland).

`cargo test` here includes a test that runs a cell in a real Python kernel; it
skips when sandhilld isn't built or no Python kernel is installed.
