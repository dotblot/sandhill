# Editor and kernels

Settled decisions about the editor, notebooks, kernels, the Console, Plots,
completion, Help and the data explorer. Moved here unchanged from
`CLAUDE.md` on 2026-11-15 (one long bullet was divided by subject, sentence
by sentence); where an older plan differs, this wins.

## The editor
- Editor actions and pane height controls share one 29px row, not a second
  toolbar beneath the pane title. Single-editor documents show their size
  at its right, before the outline button ("30,001 ln", CSV/TSV "· 41 col")
  and, while text is selected, "· 12 ln selected" or on one line "· 3 col
  selected" (CSV/TSV columns, else characters; Huan, 2026-11-14).
- Opening documents
  never changes the pointer; the editor shows a thin progress bar instead
  (Huan, 2026-11-11). A Wrap toggle in the editor's action row wraps long
  lines, remembered per file type on this Mac; Markdown and plain text wrap
  by default, code does not (Huan, 2026-11-13).
- Brackets
  and quotes close themselves, brackets are coloured by depth.

## Notebook cells and outputs
- Clicking notebook code or output strengthens both frame borders without
  changing width or showing a left selection stripe. Text outputs use I-beam
  and retain selection/copy. Console and AI inputs remain transparent with a
  slightly stronger neutral inactive border and brand accent when focused.
  Both can be made taller by dragging their top edge (Huan, 2026-11-16),
  taking room from the pane above (at most 60% of the window); a double
  click on the edge puts the box back. The height is kept on this Mac.
  The line between a code cell and its outputs reads `#3  Output [5]
  0.2s` (no "Cell" word; Huan, 2026-11-14); the gutter holds no number. The wheel over an
  output box scrolls the box while it can move, then the notebook.
- When a notebook cell's current code differs from the code that made its
  output, the run line adds "edited since run" in the warning colour.
  Clicking it shows removed and added lines. Trailing whitespace on each
  line and blank lines at the end do not count; undoing the edit removes the
  label. The code comes from the run or a Quarto or R Markdown output
  sidecar. If the code is unknown, as for Jupyter outputs made elsewhere,
  nothing is marked. Sandhill does not rerun anything or change notebook
  metadata (Huan, 2026-11-18).
- Plain output/table text owns a local selection invalidation boundary
  (`output_text.rs`); keep drag highlight updates local rather than polling or
  refreshing the whole notebook. Verify while the mouse is still held down.
- The Console's code for each run can be selected and copied, keeping its
  colours; right-click copies the selection, else the run's code (Huan,
  2026-11-16).

## Kernels and the Console
- Kernel status is a dot inside the picker,
  after the kernel name. The Console's picker shows its bound file and kernel
  together; it lists the bound file's kernel, the device's other sessions and
  new kernels. Activating or running a document, or clicking a Kernels row,
  moves the Console to that session; Variables, Help and Packages show the
  Console's kernel, so opening a file without one keeps them (Huan,
  2026-11-13). A Kernels row is one line: icon, name and version, then the
  open files using it. Above 80% of device memory used by
  kernels, warn once per crossing.
- The Files
  pane has no toolbar row: sort, new and refresh sit in its tab bar. The
  Console keeps an unsent draft per session, and its picker sits against
  the restart button.
- Kernel names (Huan, 2026-11-15) have one format everywhere: language,
  version, environment in brackets, and for R the kernel after a dot:
  "R 4.6.2 · Ark", "R 4.4.1 (omicverse) · IRkernel", "Python 3.12
  (omicverse)", "Python 3.13". Python shows two parts of its version, R
  three; no environment, no brackets. The kernel's own `display_name` is
  never shown; a kernel that isn't running gets its version from its R or
  Python, asked once by the daemon. Kernel pickers drop the grey
  environment line when the name already shows it.
- Full status/errors remain in tooltips. List only runnable kernels, including
  conda/mamba kernels; do not add a separate environment inventory or disabled
  rows for environments without a kernel (Huan, 2026-11-08). This is the
  pickers' rule: Welcome → Getting Ready lists conda environments with
  Python but no ipykernel, only to offer Add ipykernel (Huan, 2026-11-19;
  `docs/design/python-ready-plan.md`). Kernels whose
  program is gone (left behind by an upgraded or removed R or Python) are
  not listed; a document that chose one still gets the "wasn't found" error
  rather than another kernel (Huan, 2026-11-15). An R without
  IRkernel (Ark doesn't count) gets an "Install IRkernel for R x.y…" action
  below the picker's kernels and in project settings; a confirmation says
  what changes, it runs as a job, and Sandhill never uninstalls it (Huan,
  2026-11-14; `docs/design/done/irkernel-install-plan.md`).
  Kernel recovery: live session, then the document's remembered choice, then
  project default. Missing remembered choices never silently change environment.
- Console kernel selection starts its session automatically; no Start button.
- Kernels are started in their own process group and must outlive the daemon.
  Shutting one down asks it to stop and, if a run keeps it busy, interrupts
  that run after a second; that is still graceful, and killing it stays a
  separately confirmed choice. A kernel that loses one of its ports to
  something else before binding it is started again, up to twice (2026-11-14).
- Code that asks something (R's `browser()`, `readline()`, `menu()`;
  Python's `input()`, `getpass()`, pdb, `breakpoint()`, `%debug`) is
  answered in the Console: it shows the prompt above its input, Enter sends
  the answer, and a password goes in a hidden field. A tool's quiet run that
  asks is interrupted; an agent is told its code waits for the user. A
  visual debugger (breakpoints in the editor) stays out (Huan, 2026-11-17;
  `docs/design/done/console-input-plan.md`).
- The Packages pane (Huan, 2026-11-20;
  `docs/design/clean-footprint-plan.md`) installs, updates and removes
  through the same confirmation as Getting Ready (what, tool, folder, the
  exact command) as a job with progress, never by typing code into the
  Console. Installs go in Sandhill's record with the reason "From the
  Packages pane".
- With "Everything in one folder" chosen (Huan, 2026-11-20;
  `docs/design/clean-footprint-plan.md`), kernel pickers list only
  Sandhill's own kernels, and kernels, the Terminal, jobs and the
  agents' commands use only its R and Python: their libraries, caches
  and settings live in its folder, and the home folder's `.Renviron` and
  `.Rprofile` are not read (a project's own `.Rprofile` still runs). A
  document asking for another kernel gets the "wasn't found" message,
  which says the R and Python page can switch.

## Plots
- Console plots stay inline unless sent to
  Plots (per plot or all; auto-send is an opt-in switch). The plot zoom and
  save window (Huan, 2026-11-14; `docs/design/done/plot-save-window-plan.md`)
  keeps the preview full height with a 260px settings column on the right
  (under it when narrow), hidden or shown by a title-row button remembered
  on this Mac; a PDF can be saved rasterized (matplotlib's data layers, R's
  whole plot).
- "Save recipe beside it" starts off and is remembered on this Mac
  (Huan, 2026-11-18). With it on, replacing a figure also replaces its
  recipe after the figure's confirmation.

## Completion, documentation and Help
- Code completion (Huan,
  2026-11-13): a plain word's first two letters open the list after a
  3-second pause (Tab opens it at once), the third letter and letters after
  `. $ @ :: ( , [` at once; accepted functions get `()` with the cursor
  inside (Ark's snippets become plain text); the signature hint sits above
  the caret line, never under the title bar or over an open menu.
- Completion (Huan, 2026-11-14): a lettered, coloured
  badge before each item says its kind; the list is placed by its own size
  and its documentation sits beside it in a fixed panel (the user drags its
  corner), so browsing never moves either; neither enters the title or
  status bar. Documentation and Help pages show prose in the interface font
  and code in the code font with highlighting, as AI replies do (plain
  docstrings are converted; R help `<pre>` blocks are R code). Code is one
  pixel smaller than the prose beside it (popups 13/12px, Help 15/14px), as
  the code font is wider (Huan, 2026-11-15). The hover popup's corner away
  from the text drags to resize it for the session. Every suggestion shows
  its documentation: items that come without it (Ark's, kernels') are
  looked up once selected; no "Live kernel" label. While typing, the
  documentation beside the list waits until 3 seconds pass without typing;
  Tab shows it at once (Huan, 2026-11-15). F1 (fn+F1) on a name in a
  script, cell or the Console opens its page in Help, from the file's own
  kernel (the Console moves to it), else the Console's.
- The explanation box (hover) shows only while the pointer rests 3 seconds
  on a name's own letters, never for the blank space past a line's end,
  and covers that name only, not the list or call around it; leaving the
  name closes it (Huan, 2026-11-16). In completion lists, names in the
  Variables pane or assigned in the document come first (Huan, 2026-11-16).
- Editor details (Huan, 2026-11-16): Python's explanation box shows its
  signature coloured as code, like R's (Sandhill asks the language server
  for Markdown). Files and folders are offered only when Tab asks, never
  while typing in a string. R's `...` is never offered as an argument and
  a call whose only parameter is `...` (`c(`) or that has none shows no
  hint. Enter inside an open call or list lines the new line up under the
  first argument, or one step in when the bracket ends its line (R 2,
  Python 4 spaces); after the call closes, the next line goes back to its
  statement's indent.

## Data explorer
- The data explorer's column edges drag to resize (Huan, 2026-11-14): no
  type line under names; each header's ⋮ menu (Copy, Select, Pin/Unpin,
  Sort Ascending/Descending with ⇧ adding a key, Clear Sorting, Add Filter)
  replaces click-to-sort; pinned columns stay left while scrolling sideways.
  The project keeps each table's pins and dragged widths with its sorting,
  filters and hidden columns, by column name; Reset View forgets them.
  The action row (Huan, 2026-11-14; `docs/design/done/data-explorer-redesign-plan.md`)
  holds, left, the column panel toggle, the table's name (code on hover) and
  a status dot (green live, amber running, red error); right, in a fixed
  order, Search, Filter ("Filter · 2" with filters on), one Export menu
  (Copy as Code, Copy Filters as Code dimmed without filters, Save as CSV…),
  Read again, Open in a new window and an icon-only gear (column panel by
  default, Reset View). Menus open under their buttons. While filters or
  sorting are on, a row of wrapping chips sits over the table (filters
  change, sort keys flip, × removes) with Clear all. The column panel has a
  COLUMNS heading with "6 of 7" and a ⋯ menu (Show/Hide All Columns), a Find
  column field, then one 32px line per column: fold arrow, type badge
  (num, int, str, cat, bool, date, other), name, sparkline, missing share
  (count on hover), and the shown checkbox last. Folded statistics: a wide
  histogram between min and max with mean and median, or each common value
  with a share bar (click filters to it), then "N distinct · N missing".
  Clicking a column scrolls the table to it. Columns start as wide as their
  first rows' measured values (at most 320px); a whole-column selection
  uses the lighter tint.
