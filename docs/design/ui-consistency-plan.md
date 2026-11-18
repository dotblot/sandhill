# Making the interface consistent

Huan, 2026-11-15. Status: approved (12 and 12.5px labels become 13px;
per-row questions stay in the row). Pane tab bars first, as Huan asked.

Done so far:

- **Pane tab bars** (2026-11-15): every bar has the same backdrop,
  whether its content scrolls under it or sits below (Terminal, Plots,
  Viewer, a collapsed pane and the AI pane were greyer); tabs have one
  padding (8px; the left panes had 7, the Console 10); the kernel picker
  is a round capsule with the neutral wash and no edge (it was a 6px box
  with a navy border); the AI pane's title is tab-sized, its icons sit as
  close as the other bars', and "New conversation" no longer shows a
  second +.
- **UI values and buttons**: `app/src/look.rs` names the shared values
  for later pane passes. Labelled actions use Kit's button at 20, 26,
  or 30px, and destructive actions share one red button style.

Huan: "It seems like a lot of inconsistent." This plan keeps the look that
is settled (glass window, floating panes, round tabs, navy accent, fonts,
icons) and removes the differences that crept in while panes were built
one at a time. Nothing is restyled; things that should match are made to.

## What was measured (2026-11-15, `gpui/src`)

| What | Found | Should be |
|---|---|---|
| Text sizes | 15 different sizes (7.5 to 24px); 12, 12.5, 13 and 14 used side by side for the same kind of label | 5 named sizes |
| Buttons | Kit's button in 15 files, plus 6 hand-made kinds (Kernels, AI pane, Git diff, Welcome, cells) with their own height, corner and colours | one button, 3 sizes |
| Button and row heights | 14 heights between 16 and 36px (25, 26, 27 and 28 all in use) | 3 control heights, 2 row heights |
| Corners | 13 radii (1 to 12px); 6px in 50 places, the 12px "one radius for everything" in 72 | 3 radii |
| Hover tint | 12 strengths of the same grey | 2 (rows, buttons) |
| Icon sizes | 11, 12, 13, 14, 15, 16, 18, 22px | 3 |
| Gaps | 1, 2, 4, 5, 6, 7, 8, 10, 12, 14px | steps of 4, plus 2 and 6 |
| "Are you sure?" questions | 4 styles: card (quit, close file), a row that turns into a question (Kernels, Jobs, Packages), a red "Delete?" chip (AI history), Kit's dialog (Git) | 2: card for big, in-row for small |
| Red buttons | two label colours (white in AI history, dark red elsewhere) | one |

## The rules

One place (`look.rs`) names every value; panes use the names, never a
bare number.

- **Text**: Title 16 (card and page titles), Body 15 (window text, Help
  prose), Label 14 (tabs, toolbar buttons), Small 13 (rows, popups, menus,
  notes), Tiny 11 (badges, counts). Code stays 15, and 1px under the prose
  beside it in documentation. Welcome's 24px heading is the one exception.
- **Buttons**: one button everywhere, Kit's, in three sizes: 20px (inside
  rows and tool rows), 26px (cards, forms, popups), 30px (Welcome). Kinds:
  primary (navy), plain (outline), quiet (no outline, for icons), red.
  The hand-made kinds go.
- **Rows**: 28px for lists the user picks from (Files, Kernels, Jobs,
  Packages, Git, history, menus); 22px for dense data (Variables, tables).
- **Corners**: 12px panes, cards and popups; 6px buttons, inputs, rows and
  chips; fully round tabs, capsules and close buttons. 4px only inside
  code and tables.
- **Hover**: one tint for rows, one a little stronger for buttons and
  icons; the picked row uses the accent wash, as now.
- **Icons**: 14px in rows and buttons, 12px in dense rows and badges, 16px
  in the title bar and cards.
- **Gaps**: 4 between an icon and its label, 8 between controls, 12
  between groups; pane padding 12 across, 8 down.
- **Questions**: anything that stops running work or loses data for the
  whole file, project or app asks in a card (quit, close file, close
  project). Anything about one row (shut down a kernel, stop a job, remove
  a package, delete a conversation, delete a file) asks in that row: the
  name becomes the question, with Cancel and a red button at the right.
  Git keeps its dialog only where it needs a text field.
- **Wording of questions**: "Verb name?" ("Shut down Ark R 4.6.2?"), the
  red button repeats the verb, Cancel is always the other one, on its
  left.

## Order of work

Each step is one commit and is checked on screen in both themes before
the next.

1. Names in `look.rs` for text sizes, heights, radii, gaps, hover and icon
   sizes; no visible change.
2. Buttons: replace the 6 hand-made kinds; one red.
3. Text sizes: move every bare number to a name (12 and 12.5 become 13).
4. Rows, corners, hover and icons, pane by pane: left panes, bottom panes,
   AI pane, Welcome and Manage pages, cards and popups.
5. Questions: AI history's "Delete?" chip, Jobs' "Stop?" and Packages'
   "Remove?" follow the Kernels row pattern with the name in the question.
6. A page in `docs/design/ui/` showing every rule, and the rules added to
   `CLAUDE.md`.

## Not changed

The window's glass, pane layout, tab shape, colours, fonts, icon art, the
code editor, the data explorer's own layout, and the terminal.

## Questions for Huan

1. Which places bother you most? They go first.
2. 12 and 12.5px labels become 13px (slightly larger). The other way,
   13 down to 12, breaks the "popups are 13px" rule. Agreed?
3. Small per-row questions stay in the row (no card). Agreed?
