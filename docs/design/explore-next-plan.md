# Explore a Table: Getting Ready, layout and the AI panel

Huan, 2026-11-19. Status: approved by Huan (2026-11-19): packages stay in
the user's own R library with a record of what Sandhill added (1.2);
Getting Ready installs and removes Claude Code and Codex (1.3); panels
move beside, above or below each other (2); the AI panel is built now,
with its switch (3).

Huan asked (2026-11-19) for four changes to Explore a Table. One was a
bug, fixed already (commit 5573a32): only the first sample could be given
a group, and Import a Sample Sheet… was added. This plan covers the other
three. Each part ships on its own.

## 1. Getting Ready shows what is there, and can undo what it did

### What the page shows
Always the full card, also when everything is ready (no more single green
line). One box per item, each with a green, amber or red mark:

| Item | Shown |
|---|---|
| R | Version, where it came from (CRAN's installer, Homebrew, conda, or "installed another way"), and its path. How packages install for it: ready-built (CRAN's R) or built on this Mac (Homebrew, conda). |
| Where packages go | The library Sandhill installs into, as a path with Show in Finder. |
| R kernel | The kernel Explore a Table uses and where IRkernel is. |
| R packages | Each package with its version and where it is: "in your R library" or "installed by Sandhill". |
| AI helpers | Claude Code and Codex: installed or not, version, signed in or not (section 1.3). |

Where R came from is read from its path: `/Library/Frameworks/R.framework`
is CRAN's; `/opt/homebrew` or `/usr/local/Cellar` is Homebrew's; a folder
with `conda-meta` is conda's.

### 1.2 A record of what Sandhill installed, so it can undo it
Packages keep going into the user's own R library (Huan, 2026-11-19), and
Sandhill writes down what it added:
- Each install job lists the library's packages before and after, and
  appends what is new (name, version, library, date) to a record on the
  device, `<state>/installed-by-sandhill.tsv`. Dependencies pulled in come
  with it. IRkernel installs made by Sandhill (the package when it was
  missing, and the kernel file Sandhill wrote) are recorded the same way.
- The R packages row marks each package "installed by Sandhill" or "yours".
- **Remove What Sandhill Installed…** asks in its row (the destructive
  pattern: "Remove what Sandhill installed?", Cancel, then the red Remove)
  and lists what goes. It warns: if you've started using these in your own
  R work, it will need them installed again.
- A recorded package whose version has changed since (the user updated or
  reinstalled it) is theirs now and stays. R, Python, the user's own
  packages, kernels and agents are never touched. Agents' settings and
  logins (`~/.claude`, `~/.codex`) stay too; only the programs go.
- Packages installed before the record existed can't be told apart and
  stay. On Huan's Mac the phase 1 button installed nothing.

### 1.3 AI helpers
- For Claude Code and Codex: installed (with version), and signed in.
  Signing in stays each agent's own: the button opens Welcome → Agents,
  where Sign In already is. Sandhill never sees a login.
- Not installed: **Install** shows the agent's own install command and
  what it does, then runs it as a job, as the IRkernel install does:
  - Claude Code: Anthropic's installer, `curl -fsSL
    https://claude.ai/install.sh | bash` (into `~/.local`).
  - Codex: `npm install -g @openai/codex` when Node's `npm` is there;
    without it, the row says Codex needs Node.js and links to nodejs.org.
- Sandhill records the agents it installed, so Remove What Sandhill
  Installed… can remove them too (Claude Code's files in `~/.local`, or
  `npm uninstall -g @openai/codex`).
- Getting Ready never asks an agent anything on its own: the version comes
  from the program on disk, and "signed in" from what the Agents page last
  saved (the rule from 2026-11-14).

## 2. Panels that resize and move
- The Data Explorer, Plots, the Guide and the AI panel (section 3) have
  splitters between them that drag to resize, as in the main window.
- Each panel's header drags. Dropping it on the left or right half of
  another panel puts it beside that one; on its top or bottom half, above
  or below it in the same column. An outline shows where it will go.
- Two columns or three, with up to two panels in a column.
- The layout is this Mac's, the same for every Explore a Table window, and
  stays after quitting. The title bar's ⋯ menu has Reset Layout.
- Built in `app/src/view/quick.rs` with a small layout model in
  `app/src/quick/layout.rs` (columns of panels and their shares), tested
  without a window: moving a panel, resizing, keeping each at least
  240px, and reading a saved layout from an older version.

## 3. The AI panel, and turning it off
The AI panel isn't built yet: it is phase 2 of the Explore a Table plan
(`docs/design/quick-tools-plan.md`, section 7). This part builds it, with
the switch Huan asked for:
- A title-bar toggle shows or hides the AI panel, as the main window's
  does. Hidden, the other panels take its room. The choice is this Mac's,
  for every Explore a Table window.
- It starts hidden when neither Claude Code nor Codex is ready, with "Set
  up an AI helper" in the Guide pointing to Getting Ready.
- Everything else in section 7 of the Explore plan as approved: the line
  naming where messages go before the first one, the tables as visible @
  chips, Sandhill's Explore guide as the agent's instruction, the
  `quick_state`, `quick_suggest` and `quick_make` tools, and suggestions
  that take a click before a plot uses them.

## Settled rules this changes
- `docs/decisions/window.md`: Getting Ready shows the full card always;
  Explore a Table's panels resize and move, and its AI panel can be hidden.
- `docs/decisions/devices.md`: Sandhill records the R packages, kernel
  files and agents it installs, and can remove exactly those.
- `docs/decisions/agents.md`: Getting Ready can install Claude Code and
  Codex with their own install commands, after showing them.

## Order
1. Getting Ready (section 1), as it changes where packages go.
2. Panels (section 2).
3. The AI panel (section 3), the largest.

## Checks
- Daemon tests: where R came from, for CRAN, Homebrew and conda paths;
  installing a package into Sandhill's library with a real R into a
  temporary home, then removing it, leaving the user's library unchanged;
  the "R for Sandhill" kernel starts and finds packages in both libraries.
- App tests: the layout model (section 2).
- On this Mac, after Huan agrees to a test build: the card, Remove, the
  layout dragging and the AI toggle on screen.
