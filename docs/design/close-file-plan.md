# Closing a file with a live kernel or AI conversation

Huan, 2026-11-15. Status: approved and built. Boxes start unticked;
conversations count when pinned to the file or when it was @-mentioned,
each listed with its own box.

## Today

Closing a tab asks only when it has unsaved edits ("Save changes to x?
Running code keeps running either way."). A kernel the file used keeps
running, and so does any AI conversation working on it. Both show
elsewhere (Kernels, the AI pane's history), so nothing is lost, but
nothing says so at the moment of closing.

## What counts as linked

- **Kernel**: the file's live session (the one its runs go to).
- **AI conversation**: a running one about the file: pinned to it
  (Explain/Improve this notebook pin it), given it at the start, working
  in it now, or with it @-mentioned in any message. Each is listed with
  its own box, so the user picks which to stop. Conversations about the
  project in general are not asked about.

## The question

One card, the quit card's style (380px, full-width stacked buttons), shown
only when something is linked and live. Unsaved edits fold into the same
card instead of a second question.

```
┌──────────────────────────────────────────┐
│ (crane)  Close qc.ipynb?                 │
│ Its kernel and an AI conversation are    │
│ still running.                           │
│                                          │
│ ☐ Shut down Python 3.12 (omicverse)      │
│   Also used by: de.ipynb                 │
│ ☐ Stop "Improve this notebook" (Codex)   │
│                                          │
│ [   Save and Close   ]  (default)        │
│ [   Don't Save       ]  (only if unsaved)│
│ [   Cancel           ]                   │
└──────────────────────────────────────────┘
```

- Both boxes start unticked: closing keeps background work, as quitting
  does by default. Ticking one shuts the kernel down or stops the
  conversation as the tab closes.
- A kernel still running code says so ("running a cell") beside its box.
- A kernel other open files or the Console also use names them; shutting
  it down ends their session too, so the box says so.
- A stopped conversation stays in the AI pane's history and can be
  resumed later, as now.
- No unsaved edits: the button reads "Close".
- Escape cancels; Return takes the default.

## Not changed

Closing a project or quitting keep their own questions. Media tabs, data
explorers and files with no live kernel or conversation close as they do
now.
