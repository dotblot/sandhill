# Installing IRkernel from Sandhill (approved by Huan, 2026-11-14; built)

Huan, 2026-11-14: IRkernel did not appear in the kernel list. Cause: this Mac
has no IRkernel at all. Homebrew's R 4.6.2 lacks the package and no `ir`
kernel is registered (only `~/Library/Jupyter/kernels/ark` and omicverse's
`python3`). Sandhill lists registered kernels only, so it was right not to show
one. Huan chose to have Sandhill offer the install.

## What Sandhill finds

The daemon looks for R installations on its device:

- `R` on the daemon's `PATH`
- CRAN's `/Library/Frameworks/R.framework` (macOS)
- conda, mamba and pixi environments that hold `bin/R`

An installation is offered IRkernel when no registered kernel's `argv`
starts with that R. Ark doesn't count, so R that runs Ark can gain IRkernel
too. Results report the R version and path, and are refreshed with the
kernel list.

## Where it is offered

There are no disabled kernel rows (the 2026-11-08 rule stays):

- Project settings, R row: "R 4.6.2 (/opt/homebrew/bin/R) has no IRkernel."
  with an **Install IRkernel** button.
- The kernel picker (Console and editor): one action at the bottom,
  **Install IRkernel for R 4.6.2…**, below the kernels and set apart from
  them. Absent when every R has IRkernel.

## Installing

A confirmation states what changes before anything runs:

- **Plain R** (Homebrew, CRAN, other): run
  `install.packages("IRkernel")` from `https://cloud.r-project.org` into that
  R's first writable library, or its user library if none is writable. Then
  Sandhill writes the kernel registration itself, so `jupyter` isn't needed:
  `kernel.json` in the user's Jupyter kernels folder
  (`~/Library/Jupyter/kernels` on macOS,
  `~/.local/share/jupyter/kernels` on Linux). It names that R by full path,
  so it never switches to another R on `PATH`. The name is `ir`, or
  `ir-<version>` if `ir` is taken by another R. The display name is
  "R 4.6.2 (IRkernel)", so it differs from Ark in the list.
- **conda/mamba/pixi environment**: use the environment's own package
  manager (`conda install -n <env> -c conda-forge r-irkernel`,
  `pixi add r-irkernel`), which registers the kernel inside the environment.
  Mixing CRAN packages into conda R breaks easily.

It runs as a job in the Jobs pane, so the progress, compiler output and errors
stay visible after the app disconnects. On servers it runs on the server.
When it finishes, the kernel list refreshes and the new kernel can be picked.
A failure leaves the job's log and changes nothing else.

Sandhill installs, never uninstalls. User R and environments are not
Sandhill-owned (device cleanup rule), so removal is left to the user.

## Checks (done)

- `crates/daemon/tests/irkernel.rs` (skips without R): an R on `PATH` is
  offered with its version and the `ir` name in the Jupyter folder. An R
  that wasn't offered is refused. Once an IRkernel registration names that
  R, it is no longer offered. The download itself isn't tested.
- In the app: Homebrew R 4.6.2 was installed from the Console picker in about
  a minute. `~/Library/Jupyter/kernels/ir` names `/opt/homebrew/bin/R`. The
  picker then listed "R 4.6.2 (IRkernel)" without that offer, and code ran in
  it with its variables in the Variables pane.

## Huan's answers

1. Placement: project settings plus the kernel picker.
2. Package source: that R's own `repos` setting, else CRAN's cloud mirror.
