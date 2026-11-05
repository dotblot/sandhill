# Plan: a GPUI trial version of Sandhill

*Written 2026-11-01. Approved by Huan the same day. Milestones 1 to 4 done 2026-11-01 (commits 608e706, b442d58, 1d50837 and ad48f22 on `gpui-experiment`; screenshots in `design/gpui/`). Milestone 5 comparison: Claude Doc https://claude.ai/artifact/93etNCcUEzY1uPSr9MMZb7 (recommends parking).*

**In one line:** build a small, separate Mac app with GPUI (the toolkit Zed is drawn with) that talks to the same `sandhilld` server as today's app. Then compare how it feels before deciding anything.

## Where it lives

- **Branch:** `gpui-experiment`. A branch is a separate copy of the code, so `main` and the current Tauri app are never touched. It has been created, and it is identical to `main` for now.
- **Folder:** `gpui/` at the top of the repo. It is its own small Rust project, so the trial's many extra libraries don't slow down or change the normal build and its automatic checks (CI).
- Nothing in `app/`, `crates/` or `CLAUDE.md` changes during the trial, except small, safe moves if we decide to share code (see below), and each of those is listed for you first.

## How the code is shared

Today's app already reaches the server only through Sandhill's own message format (the "wire protocol"). The trial does the same, so every server feature keeps working unchanged: kernels, saved outputs, ssh servers, agents.

| Piece | Today's app | GPUI trial |
|---|---|---|
| Starting and reaching the server, on the Mac or over ssh (`sandhill-link`) | used | **used as is** |
| The message client (`sandhill-proto`) | used | **used as is** |
| Server, kernels, notebooks, agents (`sandhilld`) | used | **used as is**, nothing changes |
| Turning kernel messages into outputs | `outputs.ts` (TypeScript) | Rust. The server's `outputs.rs` could be shared instead of written a third time |
| Screens: editor, panes, chat | TypeScript + SolidJS | **rewritten in Rust** |

So only the screens are new work. In today's app those are about 24,000 lines; the trial starts with a small slice of them.

## What GPUI doesn't have (and the plan for each)

- **A code editor.** Zed's editor can't be used: it is GPL-3.0, which would force Sandhill under the same license. The plan is the editor in `gpui-component` (Apache-2.0), which has syntax colors and code completion support. It is younger than CodeMirror, so typing, Chinese input (IME), undo and copy/paste get checked early.
- **A web view** for HTML outputs: plotly, DT tables, leaflet, Shiny, Quarto previews, MultiQC reports. The option is `gpui-wry`, which places Safari's engine inside the window. It is marked experimental, so this is the biggest unknown. PNG plots don't need it: they can be drawn natively.
- **Tables** (Data Explorer). `gpui-component` has a fast table that only draws visible rows. It should work, but it is untested at our sizes.
- **Terminal.** `alacritty_terminal` (Apache-2.0), the library Zed's own terminal uses.
- **Few docs.** GPUI is before version 1.0 and changes often. The trial pins exact versions so it doesn't break underneath us.

## Milestones

Each milestone ends with a short note to you saying what works and how it feels. A build is made only when you ask for one.

1. **Run one cell.** A GPUI window opens a project folder on the Mac (local server), shows its files, opens a notebook (`.ipynb` or `.qmd`), and shows its cells. Clicking Run on one cell runs it in the existing Python or R kernel and shows the text output.
2. **Edit cells and see plots.** Cells become editable with syntax colors. Run All, and PNG plots and error messages appear under cells. Outputs saved earlier reappear when the notebook is reopened.
3. **The right side.** Console and Variables panes, plus a Terminal. Connecting to your Ubuntu server over ssh.
4. **The hard parts.** HTML outputs and the Viewer through `gpui-wry`, and a Data Explorer table with a large data frame.
5. **Decide.** We compare it with the Tauri app on start-up time, typing feel, scrolling big notebooks, memory and Chinese input. Then you choose: keep going, park it, or drop it.

Milestones 1 and 2 are the cheap test of "does it feel better?". Milestone 4 is where it could fail, so if 1 and 2 don't feel clearly better, we stop there.

## What it needs from you

- An OK on this plan (change anything you like).
- Later, to see it, a Mac build when you ask. GPUI draws with Metal, so the trial can be checked here in the cloud only by compiling, not by looking at it.
