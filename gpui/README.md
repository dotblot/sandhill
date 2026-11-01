# Sandhill in GPUI (trial)

A trial of drawing Sandhill with [GPUI](https://gpui.rs), Zed's UI toolkit,
instead of a web view. The plan and milestones are in the project's
`design/gpui-plan.md`. This folder lives only on the `gpui-experiment` branch;
`main` and the Tauri app in `app/` are untouched.

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

## Running it

```sh
cargo build -p sandhill-daemon        # at the repo root, once
cd gpui
cargo run -- /path/to/project         # the folder is optional
```

On Linux GPUI also needs `libxkbcommon-dev libxkbcommon-x11-dev` and a
Vulkan driver (`mesa-vulkan-drivers` works, even without a GPU).

`cargo test` here includes a test that runs a cell in a real Python kernel; it
skips when sandhilld isn't built or no Python kernel is installed.
