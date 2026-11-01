# Sandhill in GPUI

Sandhill drawn with [GPUI](https://gpui.rs), Zed's UI toolkit, instead of a
web view. It started as a trial (milestones 1 to 4 below); on 2026-11-01 it
was chosen to replace the Tauri app in `app/`, which it will do step by step
(see `CLAUDE.md`). It keeps the Tauri app's look, with fonts 2px larger.

It reaches `sandhilld` the same way the Tauri app does, through
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
- Terminal is a real shell from the daemon (in the project folder), drawn
  with `alacritty_terminal`: colours, full-screen programs like `top`,
  Ctrl+C, paste. It keeps running when the app closes and is picked up again.
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

- `src/look.rs` holds the Tauri app's colour tokens (`app/src/styles.css`),
  light and dark, and builds gpui-kit's theme from them, so they are
  written once. It follows the system's appearance; `SANDHILL_APPEARANCE=dark`
  (or `light`) picks one, for trying it.
- Every text size is the Tauri app's plus 2px: window text 15px, code 15px
  in Maple Mono NF, small rows 13px. Maple Mono is bundled as gzipped TTF in
  `assets/fonts` (GPUI reads TrueType, not the web fonts); `make.py` there
  makes them from `app/src/fonts`.
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
