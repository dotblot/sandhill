# Working on Sandhill

Sandhill is a Mac-native IDE for R/Python bioinformatics. Read `docs/design.md`
before making architectural changes; it records the decisions and why.

## Who you work for, and how
Huan owns Sandhill and decides what gets built. Huan is new to GitHub words
like PR and CI.
- Talk plainly, with no jargon; explain a term when you must use one.
  Reply in the language Huan writes in (sometimes Chinese).
- No builds of the Mac app unless Huan asks for one.
- No tests or experiments that weren't asked for. When Huan is only asking
  or exploring, answer from what you know instead of running things.
- Before a big change, write a plan and wait for Huan to approve it (Huan
  may change it). Small fixes Huan asked for need no plan.
- Design and building are separate steps; design answers go into the
  design docs below.

## Design docs
- `docs/design.md`: architecture decisions and why (read before changing it).
- `docs/design/bioinfo-ide-framework.md`: the original design for the whole
  IDE: goals, server and kernels, Projects, panes.
- `docs/design/agents-v2-plan.md`: the AI agents plan (chat pane, Claude
  Code and Codex, @ and # mentions), modeled on Posit Assistant.
- `docs/design/gpui-plan.md`: the plan for the move to GPUI.
- `docs/design/ui/`: the UI mockup, spec, icons and brand.
These plans are history: where they differ from this file, this file wins.

## Decisions that are settled
- Mac app: GPUI (Rust, in `gpui/`), Sandhill.app with bundle id
  `dev.sandhill.ide`. It replaced the Tauri 2 app (TypeScript UI in `app/`,
  decided 2026-11-01, retired 2026-11-02; it is in the git history). It keeps
  the Tauri app's sizes, with every font size 2px larger. No Electron.
- Look (Huan, 2026-11-05): the window is frosted glass (the system blurs
  what is behind it) with the panes floating on it, as in Xcode 27. The
  editor is solid; the side panels and the Console, Terminal and Jobs under
  the editor are translucent. Pane tab bars and tool rows float over their
  scrolling content at 82% opacity, so it shows faintly beneath (Huan,
  2026-11-11); GPUI has no per-element blur. Terminals, Plots, Viewer and
  code editors keep stacked headers (Huan kept this on 2026-11-13: Kit's
  Editor resets its own padding every frame, so a floating header would
  cover the first line). Notebook cells hide line numbers;
  scripts and Quarto Source view keep them.
  Kit's Default Light/Dark themes supply neutral surfaces, semantic status,
  chart and terminal ANSI colours. Interactive accents use the crane icon's
  navy `#142844`, with a lighter blue counterpart in dark mode (Huan, 2026-11-08).
  Preserve glass opacity and fonts; file/folder icons use Sandhill's palette
  below, other icons their source colours. Code syntax keeps
  Xcode's Default colours and italic rules. Kit widgets and custom UI share
  the same brand palette; do not tint all surfaces blue or cream.
  Tabs and capsules (Huan, 2026-11-14): one shape, fully round. Pane tabs are
  20px pills in a 24px track, title-bar capsules 26px; tracks and capsules
  are a faint neutral wash with no edge, and only the picked tab is raised
  (white in light mode, text colour at 20% in dark, soft shadow, no edge).
  The active project's capsule is not raised; its name is in the text colour,
  others' muted, semibold. Close buttons are one 16px circle: on the picked
  tab after its label, on others in the icon's place under the pointer
  (hover cannot change GPUI layout, so hidden buttons reserve no room);
  unsaved tabs keep their dot after the label.
- The app reaches the daemon only through the wire protocol, by running
  `sandhilld attach --start` (later through ssh). It never opens notebooks
  or kernels itself, so local and remote work the same way.
- Quitting defaults to keeping background work. The alternate choice stops all
  Sandhill kernels, terminals, jobs and agents on connected devices before
  stopping their daemons (Huan, 2026-11-08). Save edits first; failures keep the
  app open. Never force-kill remaining processes without a separate confirmation.
  Ordinary Disconnect/Reconnect and daemon upgrades retain independent processes.
  Closing a project alone still returns to Welcome. The quit question (Huan,
  2026-11-13) is a 380px card: the crane and "Quit Sandhill?" on one row,
  one sentence naming the connected devices, then full-width buttons stacked:
  Keep Running and Quit (default), Stop All and Quit, Cancel. A stop failure
  shows one red line, per-device reasons behind "Show details", and Force
  Stop and Quit in place of Stop All. No paragraph or keyboard hint.
- Device management lists resources, affected projects, storage paths/sizes and
  per-device results. Default to the current device only. Offline is not success.
  Cleanup/uninstall preserves history by default and project `.sandhill` data.
  Only checksum/provenance-owned components may be removed; never user Python/R,
  conda environments or standalone agent CLIs. Trash the current app last, only
  after selected devices succeeded. Unverifiable leftovers must be reported.
- Folder trust (Huan, 2026-11-12): the first open of a folder asks once;
  trust is kept by the daemon on that device (never in the folder) and covers
  the folders inside. Untrusted is Restricted Mode: files, saved outputs and
  previews only; no kernels, Terminal, Jobs, Git, language servers or agents,
  enforced by the daemon. Projects opened before it existed are trusted. Tell
  agents per run where they allow it (Codex, pi); never edit an agent's
  configuration or answer its own trust question for the user.
- Daemon (`sandhilld`): Rust, tokio. Runs on Linux (systemd user service) and
  macOS (launchd agent). Kernels speak the Jupyter protocol (ipykernel for
  Python, Ark for R; IRkernel also works).
- Notebook formats: Jupyter `.ipynb` and Quarto `.qmd`/`.Rmd`. No marimo.
- Standalone files (Huan, 2026-11-07): Markdown `.md`/`.markdown`/`.mdown`/
  `.mkd` has source and rendered views; Shell `.sh`/`.bash`/`.zsh` and
  recognised shell shebangs have syntax highlighting, as do JSON, YAML,
  TOML and `.gitignore` (shell-style comments) beside plain `.txt` (Huan,
  2026-11-13; text files up to 5,000,000 bytes). None auto-starts a
  kernel or executes on opening. CSV/TSV first offers text or import into an
  existing live R/Python session; text is limited to 5,000,000 bytes, with
  coloured columns and a pinned logical header. File reads remain daemon-only.
  Bounded text reads use the established chunked `read_bytes` protocol for
  compatibility with older servers. Import choices never display internal IDs.
  Every file row shows its size after the name, including empty files.
  PDF, SVG, PNG, JPEG, GIF, WebP, BMP, TIFF and ICO open as read-only middle
  document tabs with fit/zoom and PDF pagination. Use daemon file streaming
  and the existing tunnel, PDFKit/AppKit on macOS, and inert SVG rasterization
  without external references. Opening previews never starts a kernel.
  Every file type in the icon catalog has a defined way to open (Huan,
  2026-11-13; `docs/design/file-opening-plan.md`): text kinds open with
  grammar, bioinformatics (FASTA/FASTQ, tab-separated VCF/BED/GFF/SAM…) or
  keyword (Nextflow, WDL, BibTeX) highlighting; binary kinds ask to use the
  default app (servers: download a copy first). Unknown files under 5 MB that
  look like text open as plain text; text over 5 MB opens its first 5 MB
  read-only. The Files right-click menu offers "Open in Sandhill" and the
  default app for every file. Rendered Markdown (files, notebook and Quarto
  text cells) shows tables and images: project images through the daemon
  (none outside the project), `https://` images, notebook attachments.
- Editor actions and pane height controls share one 29px row, not a second
  toolbar beneath the pane title. Kernel status is a dot inside the picker,
  after the kernel name. The Console's picker shows its bound file and kernel
  together; it lists the bound file's kernel, the device's other sessions and
  new kernels. Activating or running a document, or clicking a Kernels row,
  moves the Console to that session; Variables, Help and Packages show the
  Console's kernel, so opening a file without one keeps them (Huan,
  2026-11-13). A Kernels row is one line: icon, name and version, then the
  open files using it. Above 80% of device memory used by
  kernels, warn once per crossing. Console plots stay inline unless sent to
  Plots (per plot or all; auto-send is an opt-in switch). Opening documents
  never changes the pointer; the editor shows a thin progress bar instead
  (Huan, 2026-11-11). A Wrap toggle in the editor's action row wraps long
  lines, remembered per file type on this Mac; Markdown and plain text wrap
  by default, code does not (Huan, 2026-11-13). Code completion (Huan,
  2026-11-13): a plain word's first two letters open the list after a
  3-second pause (Tab opens it at once), the third letter and letters after
  `. $ @ :: ( , [` at once; accepted functions get `()` with the cursor
  inside (Ark's snippets become plain text); the signature hint sits above
  the caret line, never under the title bar or over an open menu. Brackets
  and quotes close themselves, brackets are coloured by depth. The Files
  pane has no toolbar row: sort, new and refresh sit in its tab bar. The
  Console keeps an unsent draft per session, and its picker sits against
  the restart button. Completion (Huan, 2026-11-14): a lettered, coloured
  badge before each item says its kind; the list is placed by its own size
  and its documentation sits beside it in a fixed panel (the user drags its
  corner), so browsing never moves either; neither enters the title or
  status bar. Documentation and Help pages show prose in the interface font
  and code in the code font with highlighting, as AI replies do (plain
  docstrings are converted; R help `<pre>` blocks are R code). Kernel
  pickers drop the grey environment line when the name already shows it.
  The data explorer's column edges drag to resize (Huan, 2026-11-14): no
  type line under names; each header's ⋮ menu (Copy, Select, Pin/Unpin,
  Sort Ascending/Descending with ⇧ adding a key, Clear Sorting, Add Filter)
  replaces click-to-sort; pinned columns stay left while scrolling sideways.
  The toolbar has Copy as code (the whole view), Copy filters as code
  (dimmed without filters), a magnifier in Search, Open in a new window and
  an icon-only gear (summary panel by default, Reset View). The summary
  panel shows each column's missing share (count on hover), folds its
  statistics under it, and clicking a column scrolls the table to it.
  Full status/errors remain in tooltips. List only runnable kernels, including
  conda/mamba kernels; do not add a separate environment inventory or disabled
  rows for environments without a kernel (Huan, 2026-11-08). An R without
  IRkernel (Ark doesn't count) gets an "Install IRkernel for R x.y…" action
  below the picker's kernels and in project settings; a confirmation says
  what changes, it runs as a job, and Sandhill never uninstalls it (Huan,
  2026-11-14; `docs/design/irkernel-install-plan.md`).
  Kernel recovery: live session, then the document's remembered choice, then
  project default. Missing remembered choices never silently change environment.
- Clicking notebook code or output strengthens both frame borders without
  changing width or showing a left selection stripe. Text outputs use I-beam
  and retain selection/copy. Console and AI inputs remain transparent with a
  slightly stronger neutral inactive border and brand accent when focused.
  The line between a code cell and its outputs reads `Cell #3  Output [5]
  0.2s`; the gutter holds no number (Huan, 2026-11-14). The wheel over an
  output box scrolls the box while it can move, then the notebook.
- Every kernel output is journaled by the daemon so clients can disconnect and
  replay. Never make output delivery depend on a client being connected.
- Kernels are started in their own process group and must outlive the daemon.
- Window layout (Huan, 2026-11-02): three columns, 1:2:1. Left: Variables,
  Help, Packages, Kernels in one group; Files, Git, Plots, Viewer in the
  other (the two groups split 1:1). Middle: the editor, with Console,
  Terminal and Jobs under it. Right: the AI pane. Resource indicators follow
  the device connection at the left of the status bar (Huan, 2026-11-08).
  The AI pane and the right side pane are shown by default. Each pane's tab
  bar ends with a maximize button: the pane fills the space between the title
  and status bars until clicked again or Escape. A title-bar toggle between
  the left and AI toggles hides the middle column (editor with Console,
  Terminal and Jobs); the AI pane takes its width, or the left column when the
  AI pane is hidden; one column always stays, and opening or running a
  document shows the middle again (Huan, 2026-11-13). Pane height and size
  icons (Huan, 2026-11-13): fold collapses a pane to its title row, unfold
  fills its column (restoring uses the opposite motion), corners out/in
  maximize to and restore from the window. The left and middle toggles keep
  plain outline icons. The AI pane's header shows the conversation's name as
  plain text, not a tab; AI replies colour code blocks in Python, R, shell,
  JSON, YAML, TOML, Markdown, SQL, diff, HTML, CSS and JavaScript.
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
  is Devices, Projects, then Manage (Resources and Storage, Trusted
  Folders), one row selected with its page on the right; it opens on This
  Mac's page. Projects lists Pinned (the star on any device's projects),
  Open, then Running (live kernels or agents, not open), each project once.
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
  hidden-name toggle, faded files and Git/Sandhill marks.
- Popups and dropdowns all use one small text size: 11px in the Tauri
  design, so 13px in the app after the +2px rule.
  Clicking the same dropdown trigger again closes it; outside clicks and
  Escape dismiss it. Thinking effort is one segmented bar (Huan, 2026-11-12,
  after a Dribbble reference): the fill reaches the chosen level and carries
  its name, ticks divide the rest, clicks and drags snap, the fill springs and
  glows, and the strongest level is warm. No explanatory paragraph.
  Model popup: fixed 380×360px, icon-only provider tabs on a 32px left rail in
  alphabetical order with Favorites first, names in hover tooltips; the right
  model list scrolls. New conversation initially
  uses Antigravity's first listed actual model. Returned effort-variant siblings
  share one row and slider levels map to real IDs, without a second `--effort`.
  Preserve explicit provider/model selections and existing chats on refresh.
- AI conversations belong to the whole project, not to one file. `@file`
  attaches a file to that one message only; `#` picks a notebook cell
  (and brings its output). No @ symbol is shown in the composer box.
  Messages carry no implicit document context (open file, cell or
  selection); only what the user adds with @ and # (Huan, 2026-11-12).
- Console kernel selection starts its session automatically; no Start button.
  New conversation has no separate agent buttons. Agent/model selection stays
  in the composer picker, including Posit Assistant; PA manages models itself.
  Plain output/table text owns a local selection invalidation boundary
  (`output_text.rs`); keep drag highlight updates local rather than polling or
  refreshing the whole notebook. Verify while the mouse is still held down.
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
- Agent accounts (Huan, 2026-11-13; `docs/design/agent-accounts-plan.md`):
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
- Positron and Kallichore are Elastic License 2.0: do not copy their code.
  Ark and Air are MIT and may be used.
- Open source and non-commercial (Huan, 2026-11-06): Sandhill is under
  GPL-3.0-or-later (`LICENSE`, and `license` in Sandhill's own `Cargo.toml`
  files), with no paid version. Code taken from elsewhere must be under a
  GPL-3.0-compatible licence; vendored dependencies retain their own licences.
- Icons (Huan, 2026-11-06): use Gruvbox Material Icons for files/folders,
  Lucide for controls, and Developer Icons for available integration/runtime
  logos. Remove Atom Material entirely. File and folder icons keep Gruvbox's
  shapes but not its colours (Huan, 2026-11-14): `build.py` repaints each
  Gruvbox hue in the matching Xcode Default/Apple colour (blue `#41a1c0`,
  red `#fc6a5d`, yellow `#d0bf69`, green `#78c88a`, teal `#67b7a4`, orange
  `#fd8f3f`, pink `#fc5fa3`; folder body slate `#3b4558`), so icons and code
  share one palette. The light theme uses generated `*.light.svg` versions
  darkened in OKLCH, hue kept, to 4.5:1 on `#efefef` (folder emblems 3:1, as
  they sit on the dark folder body). Other vendors' logos keep their own
  colours. Formats Gruvbox
  has no artwork for are drawn for Sandhill in its document style
  (`vendor/sandhill`, GPL like Sandhill: FASTA, FASTQ, BAM, VCF, GFF, BED,
  h5ad, mtx, HDF5, Parquet, RDS, Nextflow, WDL, CWL, index files, signal
  tracks, phylogenetic trees); one icon covers a family of extensions
  (`catalog.json`); keep generic artwork
  to the fallback. A project's default icon is the project folder's.
  Vendor pinned sources with upstream licenses and notices. Keep Sandhill's
  crane: navy `#142844` with the palette's teal feather and red crown on
  neutral white `#F5F5F7`, no cream (Huan, 2026-11-14). Fonts are unchanged
  while Huan contacts the font author.
  Agent marks have no background (Huan, 2026-11-12): OpenAI's logo for Codex
  and Synthetic Sciences' mark for OpenScience, in the text colour; Posit's
  mark for Posit Assistant; Oh My Pi's gradient π; Antigravity uses his
  supplied full-colour SVG. Brand marks without a redistribution licence need
  permission documented separately from the icon-library licenses.

## Conventions
- `gpui/` is its own Cargo project (`[workspace]` of its own, own
  `Cargo.lock`, pinned `gpui-kit`/`gpui-webview` versions), so a root
  `cargo build` doesn't build it. Check it with `cargo fmt`, `cargo clippy
  --all-targets -- -D warnings` and `cargo test` inside `gpui/`. On Linux it
  needs `libxkbcommon-dev libxkbcommon-x11-dev libwebkit2gtk-4.1-dev` and X11.
  The terminal links Ghostty's libghostty-vt, built from source by
  `ghostty-vt-sys`: it needs Zig 0.17 (`ZIG=...` or `zig` on PATH) and
  access to tip.files.ghostty.org, deps.files.ghostty.org and codeberg.org.
  Where those hosts are blocked, `DOCS_RS=1 cargo clippy` still checks the
  code (it skips building the library, so nothing links).
  Current stack: Kit/Base/Component/WebView 0.9.1, Fast 0.1.15, Pre 0.3.12.
  `gpui/vendor/gpui-cn` retains the latest terminal library release (0.8.0),
  adapted to Kit 0.9; keep its upstream licences and recorded provenance.
  `gpui/vendor/gpui-component` is Component 0.9.1 through `[patch.crates-io]`
  with one change (caret menus open above the caret when they don't fit
  below; Huan, 2026-11-13; `SANDHILL.md`); drop it when upstream has it.
  `tree-sitter` stays at 0.26, Kit's own version, until Kit moves (Huan,
  2026-11-13): Cargo allows only one copy of it.
- Rust workspace at the repo root; crates live in `crates/`. The app is not
  part of it (see `gpui/` above).
- Keep crates small and focused; prefer adding a module over growing `main.rs`.
- Errors: `anyhow` in binaries, `thiserror` in libraries.
- Every daemon feature gets an integration test that uses a real kernel.
  Tests skip (with a message) when a kernel isn't installed.
- Output building (`crates/daemon/src/outputs.rs`) and finding Quarto chunks
  and their ids (`crates/daemon/src/qmd.rs`) live in the daemon crate, and
  the app uses them from there. A change gets a case in
  `tests/fixtures/outputs.json` or `tests/fixtures/chunks.json`.
- Icons are generated by `docs/design/ui/icons/build.py` from `catalog.json`,
  the two source maps and pinned original SVGs under `vendor/`. Change those
  inputs, not generated SVGs; the app embeds the output, and the mockup uses
  the same art. The crane and app icon sources remain in
  `docs/design/ui/brand/build.py`; then `node gpui/assets/icon/make.mjs`.
- Run `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings` and
  `cargo test --workspace` before committing, and the `gpui/` checks above
  when the app changed.
- The version stays 0.1.0 until Huan judges Sandhill stable, with no
  obvious bugs (2026-11-13). It is written in `Cargo.toml`,
  `gpui/Cargo.toml` and the bundle `Info.plist` in
  `.github/workflows/gpui-mac.yml`; change all three together.
