# Window

Settled decisions about the window, its panes, the title bar and Welcome.
Moved here unchanged from `CLAUDE.md` on 2026-11-15; where an older plan
differs, this wins.

## Layout and pane sizes
- Window layout (Huan, 2026-11-02): three columns, 1:2:1. Left: Variables,
  Help, Packages, Kernels in one group; Files, Git, Plots, Viewer in the
  other (the two groups split 1:1). Middle: the editor, with Console,
  Terminal and Jobs under it. Right: the AI pane. Resource indicators follow
  the device connection at the left of the status bar (Huan, 2026-11-08).
  The status bar (Huan, 2026-11-15; `docs/design/done/status-bar-plan.md`)
  is 20px round capsules with 13px text: the device with its resources as
  numbers ("CPU 12%", red at 80%), then Restricted in amber, a running
  task with × to cancel, the latest message (problems red with × to
  dismiss, full text on hover), and at the right each agent's limits as
  what is left ("5h 62%  7d 88%", green/amber/red, "–" until checked). A
  chevron hides the resources, and hidden they are not measured at all
  (this Mac's choice, for every device). Narrow windows drop the resource
  numbers, then the agents' numbers.
  The device popup (Huan, 2026-11-16): the device, its state and one grey
  line (sandhilld's version, how long it has run, for another computer its
  address), its usage as rings (a click opens that resource's details),
  its running kernels with the files using them, a "Show usage in status
  bar" switch, then the connection's actions and Sandhill's version. The
  switch is on by default; off, the status bar has neither the numbers nor
  the chevron, and usage is measured only while the popup is open.
  The AI pane and the right side pane are shown by default. Each pane's tab
  bar ends with a maximize button: the pane fills the space between the title
  and status bars until clicked again or Escape. A title-bar toggle between
  the left and AI toggles hides the middle column (editor with Console,
  Terminal and Jobs); the AI pane takes its width, or the left column when the
  AI pane is hidden; one column always stays, and opening or running a
  document shows the middle again (Huan, 2026-11-13). Pane height and size
  icons (Huan, 2026-11-13, redrawn 2026-11-14 after his RStudio-like
  references): a short window (title row only) collapses a pane to its
  title row, a tall window with a title bar fills its column, and two
  overlapping windows restore the split, so the two buttons never match;
  corners out/in
  maximize to and restore from the window. The left and middle toggles keep
  plain outline icons. The AI pane's header shows the conversation's name as
  plain text, not a tab; AI replies colour code blocks in Python, R, shell,
  JSON, YAML, TOML, Markdown, SQL, diff, HTML, CSS and JavaScript.

## Title bar, windows and Welcome
- Title bar (Huan, 2026-11-06): one capsule per project, including multiple
  projects on the same device. The active capsule shows the project name,
  a divider and open file tabs; inactive ones show the name and file icons.
  Each project name has a local-device or remote-server icon. File labels
  are not repeated in the editor pane. Left panel controls and the right
  theme switch have no enclosing capsule. The left status bar shows the
  active project's device and connection state. Windows (Huan, 2026-11-12):
  exactly one maximized main window holds the projects as capsules; Welcome
  is its own window, as wide as its sidebar and widest page (1036px; Huan,
  2026-11-13, after half the display left empty space beside the lists) and
  half the display's height, at least 800×600 and never resized smaller,
  never drawn inside the main window. Sandhill
  starts with the projects open at the last quit, else with Welcome alone;
  `+` brings Welcome up beside the main window, as does the house button
  beside the theme switch (Huan, 2026-11-13), and the project chosen opens
  there, closing Welcome; closing the main window (or its last project)
  brings Welcome back; closing Welcome alone quits (with the background-work
  question). Welcome's sidebar and content reach the window top; the sidebar
  is Start (Recent, Explore a Table, R and Python), Devices, then Manage
  (General, Resources and Storage, Trusted Folders, Agents), one row
  selected with its page on the right; it opens on Recent, or on Explore a
  Table when nothing is recent (Huan, 2026-11-19: Start replaced the
  Projects row under Devices). A device has one name everywhere (Huan,
  2026-11-15): the one given it, else "This Mac" ("This computer" off a
  Mac) or the server's ssh name, in Welcome, the status bar, title-bar
  capsules, cards and menus. Recent lists the tables explored, then Pinned
  (the star on any device's projects), Open, Running (live kernels or
  agents, not open) and Opened before (the newest few), each project once.
  Rows show live kernels and agent conversations. Devices can be renamed and
  projects given a built-in folder/file icon; both are this Mac's app
  settings, never written to the device or folder. Resources and Storage is
  a Welcome page, also from a project window's device menu, never a dialog.
  Project rows (Huan, 2026-11-14) have a ⋯ menu (also on right-click): Save
  All Files, Stop All Kernels (confirmed), Close Project, Change Icon.
  Closing a project with unsaved files or running kernels asks once: Save
  and Close (default, kernels keep running), Save, Stop Kernels and Close,
  or Cancel. Removing an open project from the list asks to close it first.
  Choosing folders (Huan, 2026-11-14): on this Mac, the system's folder
  window; folders dragged from Finder onto Welcome open as projects here.
  Servers keep Sandhill's own chooser, read through the daemon, with
  places, a clickable path, filter/typed paths, keyboard, New Folder,
  hidden-name toggle, faded files and Git/Sandhill marks. Files are chosen
  the same way (Huan, 2026-11-18): Variables' Import asks for its dataset
  in the system's window on this Mac and in Sandhill's chooser, in its file
  mode, on servers, instead of a file list of its own.
- Settings (Huan, 2026-11-15): Welcome's Manage section starts with
  General, this Mac's choices for every project: the theme (System, Light,
  Dark), notebooks' runs in the Console and plots in Plots, suggesting the
  next cell, following the AI's edits, wrapping long lines per kind of
  file, the Files sort order, and whether plot windows' settings column and
  the Data Explorer's column panel show at first. ⌘, and Sandhill →
  Settings… open it; there is no separate settings window. Each choice
  also stays beside what it changes, and changing it anywhere applies to
  every open project at once. Project settings stay per project, in a
  card drawn the same way (Huan, 2026-11-15): Kernels, Running and AI
  groups, the round kernel picker with a floating list, capsule tabs for
  two-way choices, and a Save button, as it writes a file collaborators
  may share.
  The File history group (Huan, 2026-11-18) sets the days to keep, versions
  per file, largest file size and space on each computer. These are this
  Mac's choices. Sandhill sends them on connect and when they change; each
  daemon keeps the last limits it received.
- Explore a Table (Huan, 2026-11-19, renamed from Quick Tools the same
  day; `docs/design/quick-tools-plan.md`): Start → Explore a Table takes
  tables, Getting Ready checks for R, its kernel and the packages and
  installs what is missing, and Recent lists the tables explored. Opening
  a table opens a third kind of window, one per session, beside the main
  window and Welcome, titled "<name> · Explore": the Data Explorer and
  Plots side by side and the Guide (what is known, what can be made, the
  AI) on the right, with no Files, editor, Console, Terminal, Jobs, Git or
  Variables. Make Plots and Find Changed Genes work from the Guide; Find
  Enriched Pathways (GO) comes next. They run
  Sandhill's own R templates (R only); everything works without an AI
  account. Sandhill carries no R or packages. An AI suggestion takes a
  click before a plot uses it.
  Getting Ready (Huan, 2026-11-19; `docs/design/explore-next-plan.md`)
  always shows the whole card, not a single "Ready" line: R's version,
  where it came from (CRAN, Homebrew, conda or other) and its path, how it
  installs packages, R's libraries (`.libPaths()`), the kernel,
  each package with its version and whether Sandhill installed it, and
  Claude Code and Codex: installed or not, and signed in as Agents last
  saw (never asked here). Signing in stays on Welcome → Agents.
  R libraries (Huan, 2026-11-19): before installing R packages or
  IRkernel, Sandhill lists R's own libraries and asks which one, every
  time (Getting Ready and the kernel picker's Install IRkernel). It never
  creates a library (no more `R_LIBS_USER` folder made for the user);
  with no library it can write to, it says how to make one and stops.
  One exception (Huan, 2026-11-20; `docs/design/clean-footprint-plan.md`):
  a project that carries its own `renv.lock` is set up inside the
  project (`<project>/renv/library`, renv's own way), shown and asked
  first, so its pinned versions never change the user's other work. In
  "Everything in one folder" that library lives in Sandhill's folder.
  Python for notebooks (Huan, 2026-11-19; `docs/design/python-ready-plan.md`):
  a Getting Ready box between R and the AI helpers shows the Python
  kernels Sandhill finds and the mamba (else conda) it uses. It offers
  Create a Python Environment… (name, default `sandhill-python`; Python
  3.12, 3.11 or 3.13; Data analysis and Single-cell package sets, both
  ticked by default), Add ipykernel… for environments with Python but no
  kernel, and Install Miniforge… only when there is no mamba or conda.
  Each shows its exact command before it runs; packages come from
  conda-forge only, and Miniforge's installer leaves the shell's settings
  alone.
  Asking before installing (Huan, 2026-11-20;
  `docs/design/clean-footprint-plan.md`): nothing installs, updates or is
  removed in the user's own R or Python until its row has shown what
  (packages, where each comes from, versions when known), which tool
  (from those that suit the target: R's own installer, mamba/conda, pip,
  uv; the suggested one first), which folder (full path) and the exact
  command; then Cancel or the verb. Getting Ready lists the tools found
  (mamba/conda, uv, Homebrew). Without R it offers R from CRAN (Mac),
  Homebrew or a conda environment; a new Python environment can be made
  with mamba/conda or uv, in a folder the user can change; Install
  Miniforge… shows only when there is neither. Install Without Asking
  (off by default) reuses the last target and tool chosen for that
  language, only for installs into that same target, and says afterwards
  what it did; updates and removals always ask. Every install goes in
  the record with its tool, date and reason. What Sandhill Installed
  replaces Remove What Sandhill Installed: the record with ticks, none
  ticked, removing the ticked items still as Sandhill left them, each
  with the tool that installed it.
  Sandhill's own R and Python (Huan, 2026-11-20;
  `docs/design/clean-footprint-plan.md`): the first time Sandhill runs,
  Welcome asks in three steps, each with its suggested answer picked:
  where R and Python come from (what's on this computer, or everything
  in one folder, `~/.sandhill/environment` unless the user picks
  another), what the user will work on (the five kits: Bulk RNA-seq,
  Single-cell in R, Single-cell in Python, Plots and reports, Pathways
  and gene sets, with a core always included; Bulk RNA-seq and Plots and
  reports ticked), and a review (what, where, download size, free space,
  about how long) before Set Up. Set up later skips it. Until the user
  chooses, Sandhill behaves as before. The folder is made with
  micromamba from conda-forge and bioconda, and nothing outside it
  changes. Getting Ready shows the choice at its top, can switch at any
  time (switching removes nothing), adds kits, and removes the folder
  (asked in its row). GSVA, which bioconda doesn't build for
  Apple-silicon Macs, is built there with Apple's developer tools,
  offered by Install Developer Tools; without them the set-up goes on
  without it and says so.
  R and Python (Huan, 2026-11-20; `docs/design/clean-footprint-plan.md`):
  Getting Ready is renamed R and Python (Start → R and Python). It shows
  the choice above, the tools and Install Without Asking, one card per
  environment (Sandhill's own, else each R and each Python with a
  kernel), and What Sandhill Installed. A card's line says in plain
  words what it is ready for or what is missing or needs repair; opened,
  it has Add Packages (one search over CRAN, Bioconductor, conda-forge,
  bioconda and PyPI, a GitHub owner/repo or a downloaded package file,
  and kits), Updates, Check and Repair (each problem with a plain reason
  and Repair), Undo Last Change (Sandhill's last change to it), and More
  (New Environment, Duplicate, Export, Import, Delete, asked in its row).
  Every action shows its plan and command before it runs. Claude Code and
  Codex moved to Manage → Agents.
  Switching to Sandhill's own (Huan, 2026-11-20): Sandhill asks whether
  its own R and Python start with only what Sandhill needs or also copy
  the packages of the user's current R and Python. A copy first shows a
  table of where each copy comes from (conda at the same or another
  version, CRAN or Bioconductor, PyPI, GitHub, or can't be copied and
  why); the user's R and Python are only read; a report afterwards
  lists what failed, each with its reason and fix. Project Settings can
  Save This Project's Packages (`renv.lock`, `environment.yml` or
  `requirements.txt` in the project, written by Sandhill from what the
  project's code uses) and Set Up This Project's Packages from such a
  file; the missing-packages bar offers the set-up when the project
  carries one.
  The lock and Advanced (Huan, 2026-11-20; plan sections 3, 10, 11):
  while Sandhill's own R and Python are chosen, kernels, the Terminal,
  jobs and agents run under the system's sandbox (Apple's on a Mac,
  bubblewrap on Linux when it works) so they can't write into the
  user's other R, Python, conda and Homebrew; a refused write says so
  in plain words. On a Mac an agent with its own sandbox on (Codex
  outside Full access, Claude Code with its sandbox enabled) runs under
  that instead, since sandboxes can't nest there (Huan, 2026-11-21).
  R and Python → Advanced holds the download sources
  (the fastest working one chosen by Sandhill, China's Tsinghua and
  USTC mirrors included, the system's proxy followed), the suggested
  tool per language, each R's Bioconductor release, Install Without
  Asking, and Open Terminal Here. Installs check free space first;
  installs cut off part way are offered to finish or undo on the next
  start; after an R upgrade, the packages the old R had are offered for
  reinstall from their sources. The page works for servers too.
  The AI pane (Huan, 2026-11-19; `docs/design/explore-next-plan.md`) has
  a title-bar switch shared by every Explore window on this Mac. Until
  that switch is used, the pane shows only with a signed-in Claude Code
  or Codex. The other panels use the room when it is hidden. The Guide
  points to Getting Ready when neither helper is ready. Continue Without
  AI hides the pane only in the session in front.
  Explore's default layout (Huan, 2026-11-19): three columns, the Data
  Explorer on the left, Plots above the Guide in the middle, the AI on the
  right, widths 2:1:1 (2:1 while the AI is hidden). The AI pane has no
  "AI" label above it: the conversation's own header is its top, so it is
  the one panel not dragged by a header (others can still be dropped
  beside it).
  Explore's plot style (Huan, 2026-11-19): Sandhill's own plots take the
  look of PubOmixR, Huan's package (private on GitHub): `theme_classic`
  at 13 pt, a thin black frame, black axis text, no grid; up red
  `#E41A1C`, down blue `#1F78B4`, their light shades, grey for the rest;
  dashed grey cutoff lines with bold counts; enrichment dots outlined in
  black, red to white, on banded rows. When PubOmixR is installed, the
  volcano plot is its own `vizVolcano()`; recipes still run without it.
  Its enrichment functions need `runEnrichment()` output, so enrichment
  tables use the copied look only.
  Gene symbols (Huan, 2026-11-19; `docs/design/gene-symbols-plan.md`):
  for a gene column of Ensembl or Entrez IDs in a human or mouse table,
  the card offers Add Gene Symbols. It adds a `symbol` column beside the
  IDs in the session's copy (the file is never changed), from
  org.Hs.eg.db or org.Mm.eg.db via `AnnotationDbi::mapIds` (version
  endings dropped, the first symbol when there are several), says how
  many genes got one, and plots then label genes by symbol. A gene with
  no symbol has its ID in the symbol column, never NA (Huan,
  2026-11-19). Use IDs Instead goes back. The organism packages (about
  90 MB each) install on their own from Getting Ready, into the library
  the user picks.
  Find Changed Genes (Huan, 2026-11-19; `docs/design/find-changed-genes-plan.md`)
  starts from raw counts and named sample factors. Every annotation
  column in a sample sheet is a factor; the first level is the reference
  and can be moved. Ready-made designs cover one or two factors, their
  interaction and numeric dose trends. A custom formula is checked in R.
  The methods are DESeq2, edgeR and limma-voom. Batch handling can use
  fixed terms for batches or donors, SVA, RUVg, ComBat-seq or ComBat-ref.
  `removeBatchEffect()` is for PCA and heatmap values only. Each comparison
  adds a results table with its direction and gene symbols carried over.
  Omnibus tests report a fold change for a separately chosen comparison.
  The exact run is in `steps.R`; methods include the design, test, batch
  handling and package versions. The packages install on demand from
  Getting Ready into the library the user picks.
  Compressed tables (Huan, 2026-11-19; `docs/design/find-changed-genes-plan.md`):
  CSV, TSV, TAB and TXT also open gzipped. For ZIP, TAR and gzipped TAR,
  choose which tables to copy into the session; the archive stays as it is.
  For one counts file per sample, Combine into One Counts Table keeps the
  genes found in every sample, lists the gene counts, and saves a CSV
  and the recipe in `steps.R`.
- Linux (Huan, 2026-11-18): Sandhill comes to Linux by way of Ubuntu
  (`docs/design/linux-plan.md`); other Linux systems and desktops wait.
  There Sandhill draws close, minimise and maximise in its title bar, on
  the right as Ubuntu puts them, and all three always show, whatever the
  desktop's own button setting says.
- Linux on hold (Huan, 2026-11-18): the Ubuntu version works (checked on
  Huan's Ubuntu 26.04), but it waits until the Mac app is as stable as
  possible. Its code stays in the app, behind platform checks that leave
  the Mac unchanged; its GitHub check and package run only when started by
  hand, and no `.deb` is released.
