# Four ideas borrowed from other apps

Huan, 2026-11-18. Status: approved by Huan (2026-11-18): this order, the
recipe switch off until switched on, the file history limits below, and
those limits changeable in General. Built on 2026-11-18. Notebook
versions in File History are compared as readable cells (a "Cell 1 ·
code" line, then the cell's source), not as the stored JSON.

Huan liked four ideas from other tools (2026-11-18) and asked for a plan
to bring them into Sandhill. The Mac app's stability still comes first:
each part below is small and ships on its own, in this order.

| Part | Borrowed from | What it answers |
|---|---|---|
| 1. Edited since run | Observable | "Did the code I'm looking at make this output?" |
| 2. File history | VS Code's Timeline | "What did this script say yesterday?" |
| 3. Figure recipe | Galaxy's history | "How did I make Figure 3?" |
| 4. Look inside `.h5ad` | HDFView | "What's in this 20 GB file before I load it?" |

## 1. Edited since run

### What the user sees
When a notebook cell's code no longer matches the code that made its
outputs, the line between them reads `#3  Output [5]  0.2s  · edited since
run`, in the warning colour. Hovering says "The code changed after this
output was made. Run the cell to update it." Clicking the words shows the
code that ran next to the code now, in a small popup with removed and added
lines. Undoing the edit back to the code that ran removes the label, as it
compares text, not "was the cell touched".

### How
- The comparison is in the app: the cell's source now against the code
  of the run that made its outputs. Trailing spaces and blank lines at the
  end don't count.
- Where the code that ran comes from:
  - A run from this Sandhill session, or one replayed from the daemon's
    journal: `Run.code` (`app/src/workbench.rs`), found through
    `open.cell_runs`.
  - A Quarto or R Markdown document's saved outputs: the sidecar's
    `SavedCell.code` (`crates/daemon/src/sidecar.rs`), which the app gets
    with the outputs it already reads.
  - A Jupyter notebook whose outputs were made elsewhere (in Jupyter, or
    before its run left the journal): unknown, so no label. Sandhill never
    guesses.
- Drawn in `Workbench::run_line` (`app/src/editor/cell_view.rs`). The
  popup's line comparison reuses `diff(before, after)` from
  `app/src/agents/cell_ai.rs`, moved to a shared editor module since two
  features now use it.
- No change to notebook files: nothing new is written into `.ipynb`
  metadata, so Git diffs and other Jupyter tools are unaffected.

### Not included
- Marking later cells that depend on an edited one (that needs guessing
  which cells read which variables; Observable can because its language
  forces it, R and Python don't).
- Rerunning anything by itself.
- Scripts and the Console (their runs aren't attached to text that stays).

### Checks
- App unit tests: same code (with and without trailing spaces) gives no
  label; an edit gives the label; undoing it removes the label; a Jupyter
  notebook with outputs and no known run gives none.
- Daemon test with a real kernel: a Quarto cell run, saved, reopened and
  edited shows the label from its sidecar code.

## 2. File history

### What the user sees
Right-click a file in Files (or use the command palette on an open file)
and choose **File History…**. A tab opens: on the left, the file's
earlier versions, newest first, each with its time, why it was kept
("Saved", "Opened") and how many lines changed; on the right, that version
compared with the file now, removed and added lines coloured as in Git's
diff tab. Two buttons:
- **Restore This Version** puts the old text into the open editor as one
  edit: ⌘Z undoes it, and nothing is written until the user saves.
- **Copy** copies the selected lines, or the whole version.

This works without Git and on servers, as the daemon keeps the history.

### What is kept
- Each save from Sandhill keeps the text that was written.
- Before Sandhill first writes over a file, it keeps the text that was
  there, so the version from before Sandhill is never lost.
- Opening a file keeps its text when it differs from the newest kept
  version. That is how changes made outside Sandhill (an AI agent in a
  terminal, another editor) still leave a version, the next time the file
  is opened.
- Kinds: scripts, Quarto and R Markdown, Markdown and text files, and
  Jupyter notebooks. A notebook keeps its cells' code and text, not their
  outputs; restoring it brings back the code, and cells whose code changes
  lose their outputs, as an agent's edit does now.
- Limits: files up to 2 MB; versions older than 30 days are dropped, and
  at most 100 per file; identical versions are stored once; all history on
  a device together stays under 500 MB, dropping the oldest first.
- Those four numbers can be changed in Welcome → General, in a new "File
  history" group (Huan, 2026-11-18): Keep versions for (days), Versions
  per file, Largest file (MB), Space on each computer (MB), each a stepper
  like the font sizes, with the defaults above. Like the rest of General,
  they are this Mac's choices; the app sends them to every computer it is
  connected to, now and whenever they change. Each daemon keeps the last
  ones it was sent (in its state folder), so a server keeps them while the
  Mac is away. Lowering a limit drops the versions it no longer allows the
  next time that daemon tidies its history (at most a minute later).
- A renamed or moved file starts a new history (as in VS Code).

### How
- Daemon: a new `file_versions` table in `journal.sqlite` (project,
  path, time, reason, size, sha256) and the text in the daemon's
  existing `blobs/` store. Its cleanup must count these versions, so it
  never deletes a blob a version still uses.
- `Daemon::write_file` and `write_notebook`
  (`crates/daemon/src/server/document_calls.rs`) record the version; the
  document read used when a file opens records "Opened" versions.
- New protocol calls in `crates/proto`: `Call::FileVersions { path }` lists
  them, `Call::FileVersion { path, id }` returns one's text, and
  `Call::SetFileHistoryLimits { days, per_file, file_mb, total_mb }` sets
  the limits.
- App: a new `HistoryTab` in `app/src/editor/`, drawing rows the way
  `app/src/git/diff.rs` does, fed by the shared `diff(before, after)`.

### Checks
- Daemon integration test (no kernel needed): three saves give three
  versions; saving the same text twice gives one; the version before the
  first save is kept; a version older than 30 days is dropped; a 3 MB file
  is not kept; a notebook version has no outputs.
- App test: restoring puts the text in the editor without saving, and ⌘Z
  undoes it.

## 3. Figure recipe

### What the user sees
The plot save window's settings column gets one more switch, **Save
recipe beside it**, under Preview. With it on, saving `umap.png` also
writes `umap.png.recipe.md`, a short Markdown file:

```
# How umap.png was made
Saved 2026-11-18 14:02 from analysis.ipynb, cell #7, by run [12]
(2026-11-18 13:58, 4.1s).
Kernel: Python 3.12 (omicverse) on This Mac.

## Code
(the code of the run, as it ran)

## Packages loaded in the kernel
anndata 0.11.4, matplotlib 3.10.1, scanpy 1.11.2, ...

## Not recorded
Input files, and earlier cells this code relied on.
```

The "Not recorded" part is always there, so nobody mistakes the recipe
for a full guarantee. A plot from the Console says "from the Console".

### How
- `Workbench::save_plot` (`app/src/panes/plots/save.rs`) already knows the
  plot's session and run; the code is that run's `Run.code`.
- Package versions: the existing `Call::ListPackages { session }`
  (`crates/daemon/src/packages.rs`), keeping the loaded and attached ones.
  It runs quietly in the kernel; when the kernel has stopped, the recipe
  says "Packages: not recorded, the kernel had stopped".
- A plot from saved outputs with no run (a reopened notebook): the cell's
  code, marked "code as the cell reads now; it may differ from what made
  the plot" when part 1 says it was edited.
- Written with the existing `Call::WriteFile`, so it works on servers.
- The switch starts off (Huan, 2026-11-18) and is remembered on this Mac
  with the other save choices.

### Checks
- Daemon integration test with a real Python kernel and a real R kernel:
  draw a plot, save it with the recipe, and find the code and the
  matplotlib (or ggplot2) version in the recipe file.

## 4. Look inside `.h5ad`

### What the user sees
Opening an `.h5ad` file from Files shows a read-only summary tab instead
of "open in the default app":
- **Size**: 52,341 cells × 33,538 genes; X is sparse (CSR), float32,
  1.2% non-zero; layers: counts, log1p.
- **Cell table (obs)** and **gene table (var)**: each column's name and
  kind (number, text, category with its number of categories and the
  first few), then the first 50 rows.
- **Embeddings (obsm)**: names and widths, e.g. `X_umap` (2), `X_pca` (50).
- **Also inside**: varm, obsp, uns keys and whether `raw` is present.
- **Load into Python…** opens the existing import dialog's "From AnnData
  (.h5ad)" choice (`app/src/editor/imports.rs`).

Only the file's labels and small pieces are read: never the expression
matrix, so a 20 GB file opens in about a second, on a server too.

### How
- The daemon reads the file with `hdf5-pure` (pure Rust, no C library, MIT
  or Apache-2.0, active in 2026, so the Mac app and the Linux and musl
  server builds need nothing new). It is a new dependency and goes through
  `docs/decisions/licences.md` and the notices.
- New `Call::DescribeH5ad { path }` returning a summary type in
  `crates/proto`; the app's tab is a new kind in `Workbench::open_file`
  (`app/src/workbench/tabs.rs`), next to the media previews.
- The file may be from anywhere, so reading is guarded: work runs off the
  daemon's main loop, a broken or hostile file gives an error message
  instead of stopping the daemon, and reads are size-capped.
- Other single-cell files (`.h5` from 10x, `.loom`, `.rds`, Zarr) are not
  included; `.h5` and `.loom` can follow the same way later.

### Checks
- Fixtures in `tests/fixtures/h5ad/`, made by the anndata package and kept
  small (under 1 MB together): dense X, sparse CSR and CSC X, categorical
  and numeric columns, embeddings, layers, `raw`, and a file written by an
  older anndata (0.7). A daemon test checks each summary; a truncated file
  gives an error, not a crash.

## Not building
- Automatic reruns or dependency tracking between cells.
- A promise that a recipe recreates a figure (it records; it can't freeze
  input data).
- Writing to `.h5ad` files, or a general HDF5 browser.
- Any setting beyond the recipe switch and the four file history limits.

## Risks
- File history keeps copies of code on the device or server. A file with a
  password in it keeps that password in history for up to 30 days. Clearing
  the history is part of Delete All Data in the uninstall flow.
- "Edited since run" relies on knowing the code that ran. Where it isn't
  known, there is no label, so a missing label never means "certainly
  matches" for Jupyter notebooks run elsewhere.
- `hdf5-pure` might fail on some real files; the fixtures, including an
  older anndata file, are the test. If it fails them, `hdf5-reader` (MIT or
  Apache-2.0) is the replacement.
