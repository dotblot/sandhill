# Sandhill window layout: agreed design

Agreed with Huan on 2026-10-11 after 16 drafts. Draft 17 (2026-10-12) made the
text 13px instead of 14px and replaced JetBrains Mono with Maple Mono NF. The
clickable mockup is
[sandhill-layout.html](sandhill-layout.html), also published at
https://claude.ai/artifact/DSC3kDKpeh3XhX8ogsiVA9. It has light and dark
themes and four screens: Workspace, Data Explorer, With agent and Start.
The screenshots are `workspace-light.png`, `workspace-dark.png`,
`with-agent-light.png` and `with-agent-dark.png`. They show a 1452 × 860 point
window at 2× (2904 × 1720 pixels), tagged 144 dpi so they open at true size on a Mac.

The mockup is the reference. Where this spec and the mockup differ, the mockup wins.

## Window and panes
- Panes are floating, rounded, translucent glass cards on a warm gradient
  window background, in the style of macOS Liquid Glass. The left pane is not a card:
  its content sits directly on the background.
- Pane order from left to right:
  - left pane;
  - Agents, which only shows when toggled on;
  - the editor column, with the editor above and the Console below;
  - the right pane, with two groups: Variables / Help / Packages / Git on top, and Plots / Viewer below.
- Every pane reaches the top of the window. The gap is **3px**: between
  panes, from the panes to the top and right window edges, and to the left edge
  when the left pane is hidden.
- Gaps between panes are drag handles: dragging one resizes the panes on
  either side, and an orange line shows on hover. Console, Variables and Plots can be
  dragged by their tab strip to under the editor or into the right pane.
- Every pane has minimize and maximize buttons at the right end of its tab
  strip:
  - Minimized, only the tabs show and the contents are hidden.
  - Maximized, the pane fills the window, opaque, until restored.
- **One corner radius, 12px**, for every rounded thing in the window: panes,
  tabs, kernel rows, inputs, buttons and popovers.
- Active tab: the text colour mixed in at 11%, so it is darker in light mode
  and lighter in dark mode. Hover uses 6%.

## Toolbar (no title bar row, no rail)
- The toolbar floats over the top of the left pane, after the traffic lights. All the icons sit in one row with
  no gap between groups:
  - toggle left pane;
  - toggle right pane;
  - Agents, a chat icon with an orange dot when an agent waits for you;
  - search, which opens a ⌘K palette for files, symbols, commands and "search in files";
  - menu, with Connect to server, Open project, New project, Recent projects, Show console and Settings.
- When the left pane is hidden, the first pane's tabs start just after the
  toolbar icons.
- There is no settings gear elsewhere: Settings lives in the menu.

## Left pane (260px, minimum 236px)
- A segmented **Local | server** tab pair at the top. Each tab shows that machine's
  content.
- Project name row, with + (new file) and refresh at the right, then the file tree.
  Files use flat coloured icons in a muted palette.
- Kernel rows, **11px**: language icon (R, Python, Nextflow), name, what is
  attached, memory, and a trash button on hover. The trash button turns the row into an
  inline "Shut down X? Cancel / Shut down" confirmation.
- Resources, **11px**, in a 2 × 2 grid (CPU and MEM, then GPU and Disk). Each item
  shows an icon, the label and a bar of the same length for every item, with no
  percent. Items are 22px high with 8px side padding, so the content sits centred
  in the hover shading. Clicking an item opens a details popover with the percent.

## Editor
- The document bar has one kernel picker per language. Each starts with the
  language icon (R, Python), which replaces the status dot and the language name,
  then the version, kernel and session. The icon is 14px and the picker's left
  padding is 3px, so the icon sits evenly inside the rounded end.
- Run all shows no keyboard shortcut; ⌥⌘R is in its tooltip.
- The Data Explorer uses the UI font, with tabular digits, and shows column names
  exactly as written (no capitals).

## Agents pane
- Tabs show **only the provider icon**: Claude, OpenAI, or a chip for open
  models via omp. Other details:
  - A small dot on the icon shows waiting or busy.
  - The close button appears on hover.
  - The full model name is in the tooltip.
- Claude Code and Codex are their real terminals. omp is a chat view.
- A one-line context bar above the input, at **9px**. Clicking it opens the
  context breakdown just above the pointer.
- The input box is resizable by its grip and has a small send button. Its placeholder has no
  "&gt;" prefix.
- Below the input are the model + thinking-effort picker and the permission-mode picker.
  - The effort slider track is 11px thick.
  - Choosing a model maps to `/model` in the CLI; Compact maps to `/compact`.

## Right pane
- Variables can be selected and deleted to free memory; Sandhill runs `rm()`
  and `gc()` in that session.
- The session pickers in Variables and Packages start with the language icon, like
  the editor's kernel pickers, with no status dot or language name.
- Help searches by package.
- Git shows a commit graph with avatars, then staged and unstaged files with a
  commit box. Everything below its tab strip is **11px** and never bold.
- Plots has a zoom dialog: set the size or pick a preset, then copy, or save as PNG, PDF
  or SVG. Plots stay white in dark mode.

## Status bar (26px)
- Left: connection (`gpu01 · sandhilld`), R and Python kernel state, jobs.
- Right: "agent is waiting" notice, then 5-hour and weekly plan usage for
  Claude Code and Codex, then cursor position.

## Type and colour
- 13px for all UI text, in the macOS system font; 13px is the standard text size in
  macOS apps.
- Maple Mono NF (the Nerd Font build of Maple Mono, version 7.8) at 13px for all
  monospace text: the code editor and its outputs, Variables, the console and
  terminal, the agent terminals, and code in Help and Git. The Data Explorer is the
  exception: it uses the UI font. The font's Nerd Font icons let shell prompts and
  command-line tools show their symbols. Maple Mono is under the SIL Open Font
  License; the mockup carries its regular, italic, medium, semibold and bold styles.
  There is no other monospace font.
- The exceptions are the kernel and resource rows and the Git pane's contents at
  11px, and the context bar at 9px.
- Accent: `#c2570c` in light mode, `#f29a45` in dark mode.

## Open questions
- Where banked plan-usage resets come from.
- Whether more panes should be movable by dragging.
