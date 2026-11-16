# Redesigning Project settings

Huan, 2026-11-15: "redesign the project settings window. Its UI is
inconsistent to the overall style." Status: approved with Save kept, and
built the same day; checked on screen in both themes.

## What is wrong now (`welcome/project_settings.rs`)

| Now | Elsewhere in Sandhill |
|---|---|
| A 110px column of grey labels on the left, the control and its notes on the right | Welcome's General page: titled groups of rows, name and a grey line on the left, the control on the right |
| Hand-made drop-down boxes: square-ish 6px corners, a navy border while open, a white fill | The kernel picker: a round capsule with the neutral wash and no edge |
| Their list opens *inside* the card and pushes every row below it down | Every other menu floats over the window in the shared popup frame (`composer_view::popup`) |
| Two-way choices (where notebooks run, where chats are kept) are drop-downs | General's two- and three-way choices are capsule tabs (Theme, Sort files by) |
| Up to five grey notes stacked under R (environment found, only one kernel, renv, IRkernel missing, and its button) | One grey line per row |
| Title "Project settings · demo" at 16px, the file it saves to in a note under it, Cancel and Save at the bottom right | Cards (quit, close file): crane or icon with the title on one row, one sentence, buttons |

## The new card

The same card, opened from the same places (☰ menu, palette, the first
open of a project), drawn like General so the two look like one app:

- **Header:** the project's icon, "Project Settings" (16px), and the
  project's name and device in grey on the line under it. The first-open
  version asks "Which R and Python should demo use?" instead.
- **Kernels** group: an R row and a Python row. The control is the same
  round kernel picker the Console uses, and its list floats in the shared
  popup frame: Automatic (with what it picks), then the installed kernels,
  then a saved kernel this device lacks. The grey line under the name
  says one thing: what Automatic found ("From environment.yml, conda env
  omicverse"), or that only one kernel is installed. renv's note joins
  that line. A missing IRkernel becomes an "Install IRkernel…" button in
  the R row.
- **Running** group: "Notebooks and Quarto run in" with capsule tabs
  *Their folder* | *Project folder*; "Env files" with the text field.
- **AI** group: "Conversations are kept" with capsule tabs *With the
  project* | *On this device*, and the grey line saying what that means.
- **Footer:** "Saved in .sandhill/project.toml. Commit it to share with
  collaborators." in grey on the left; Cancel and Save on the right
  (Kit's buttons, 26px; the first-open version says Not now).
- 560px wide, 12px corners, scrolls inside when the window is short.

General and Project settings share the group and row drawing (moved from
`start_screen/general.rs` to `look.rs`), so they cannot drift apart again.

## Not changed

What is saved, where (`.sandhill/project.toml`), when the card asks on
first open, the notice after saving, and Restricted Mode.

## Question for Huan

General applies each choice at once. Project settings keeps its Save
button, because it writes a file into the project folder that
collaborators may get through Git, and a half-finished choice of kernel
should not be written. Agreed?
