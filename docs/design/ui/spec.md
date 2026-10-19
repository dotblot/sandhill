# Sandhill window layout: agreed design

Agreed with Huan on 2026-10-11 after 16 drafts. Draft 17 (2026-10-12) changed:
- Text is 13px instead of 14px.
- Maple Mono NF replaces JetBrains Mono for all monospace text, with code
  ligatures on; the Data Explorer uses the system font.
- The toolbar order is left pane, Agents, right pane, search, menu.
- Kernel and session pickers (editor, Variables, Packages, Console) show only
  the language icon and version; a drop-down holds the details.
- Resources are a 2 × 2 grid with no percent, and the left pane and status bar
  share one left edge.
- The Git pane's contents are 11px and never bold.
- Plan usage in the status bar is shown as rings.
- The thinking-effort control is new: a brain knob, and a star field at the top
  level.

Draft 18 (2026-10-12) replaces the Agents pane with a chat laid out like Posit
Assistant; see "Agents pane" below.

Draft 19 (2026-10-17) changed:
- Kernel rows in the left pane show no name, only the language icon and version;
  clicking a row opens its details.
- The file tree has guide lines, and folders open and close.
- The Kernels and Resources sections in the left pane can be collapsed.
- The effort knob is 50% bigger (30px); the bar keeps its height and is a little
  shorter.
- The status bar's connection item shows the machine's icon instead of a dot,
  follows the Local | server tab, and opens a connection pop-up.

Draft 20 (2026-10-18) drops every third-party logo, so the product can be sold
without asking anyone for permission; see "Icons and licences" below.

Draft 21 (2026-10-19) changed:
- A title bar row is back. The Local | server tabs move into it as browser-style
  tabs, and the toolbar icons move to its right end.
- The two side panes swap: Variables / Help / Packages / Git and Plots / Viewer
  are on the left, and the files pane (file tree, Kernels, Resources) is on the
  right.

The clickable mockup is [sandhill-layout.html](sandhill-layout.html), also
published at https://claude.ai/artifact/GVLbkapTsw8FP55ie3RESs (draft 16 stays
at https://claude.ai/artifact/DSC3kDKpeh3XhX8ogsiVA9). It has light and dark
themes and four screens: Workspace, Data Explorer, With agent and Start.
The screenshots are `workspace-light.png`, `workspace-dark.png`,
`with-agent-light.png`, `with-agent-dark.png`, `agents-history-light.png` and
`agents-history-dark.png`. They show a 1452 × 860 point
window at 2× (2904 × 1720 pixels), tagged 144 dpi so they open at true size on a Mac.

The mockup is the reference. Where this spec and the mockup differ, the mockup wins.

## Window and panes
- Panes are floating, rounded, translucent glass cards on a warm gradient
  window background, in the style of macOS Liquid Glass. The files pane is not a
  card: its content sits directly on the background.
- Top to bottom: the title bar (40px), the panes, the status bar (26px).
- Pane order from left to right:
  - the session pane, with two groups: Variables / Help / Packages / Git on top,
    and Plots / Viewer below;
  - Agents, which only shows when toggled on;
  - the editor column, with the editor above and the Console below;
  - the files pane: file tree, Kernels and Resources.
- The gap is **3px**: between panes, from the panes to the title bar and to the
  left window edge, and to the right edge when the files pane is hidden. The
  files pane reaches the right edge, with its own 6px inside. The panes sit
  directly on the status bar, with no gap below them.
- Gaps between panes are drag handles: dragging one resizes the panes on
  either side, and an orange line shows on hover. Console, Variables and Plots can be
  dragged by their tab strip to under the editor or into the session pane. With
  the session pane hidden, dropping one at the left edge of the first pane opens
  the session pane again.
- Every pane has minimize and maximize buttons at the right end of its tab
  strip:
  - Minimized, only the tabs show and the contents are hidden.
  - Maximized, the pane fills the window, opaque, until restored.
- **One corner radius, 12px**, for every rounded thing in the window: panes,
  tabs, kernel rows, inputs, buttons and popovers.
- Active tab: the text colour mixed in at 11%, so it is darker in light mode
  and lighter in dark mode. Hover uses 6%.

## Title bar (40px)
- The title bar is transparent, on the window background, and dragging its empty
  space moves the window. From left to right:
  - the traffic lights;
  - one tab per machine;
  - + (Connect to server, ⇧⌘O), which opens the Start screen;
  - empty space;
  - the toolbar icons.
- **Machine tabs** follow the style of browser tabs in a macOS title bar:
  - Each tab is 30px high with 12px corners, and at most 280px wide.
  - Each tab shows the machine's icon in a 20px rounded tile, the machine's name
    (500 weight) and the project's path in the faint colour, cut short with "…".
  - The picked tab is a raised pill: the pane background colour, a hairline edge
    and a soft shadow. The others are flat, with 6% shading on hover and a thin
    divider between two flat tabs.
  - The icon shows the connection state, like the status bar's connection item:
    plain when connected, orange when reconnecting, red with a slash when
    disconnected.
  - Clicking a tab switches the files pane and the status bar to that machine.
- **Toolbar icons**, at the right end of the title bar, all in one row with no gap
  between groups:
  - toggle the left (session) pane, ⌥⌘B;
  - Agents, a chat icon with an orange dot when an agent waits for you;
  - toggle the right (files) pane, ⌘B;
  - search, which opens a ⌘K palette for files, symbols, commands and "search in files";
  - menu, with Connect to server, Open project, New project, Recent projects, Show console and Settings.
    It opens under its button and stays inside the window.
- There is no settings gear elsewhere: Settings lives in the menu.

## Files pane (right; 260px, minimum 236px)
- It shows the machine picked in the title bar's tabs.
- Project name row, with + (new file) and refresh at the right, then the file tree.
  Files use flat coloured icons in a muted palette.
- The file tree:
  - Each level is indented 15px.
  - A 1px guide line (the text colour at 24%) runs down from a folder's chevron,
    with a 6px elbow to each child. The line stops at the last child.
  - Clicking a folder opens or closes it, and its chevron turns to point down
    when open.
- Under the tree are two sections, **Kernels** and **Resources**:
  - Each has a header row: a chevron and the name, 22px high, in the muted colour
    and semibold.
  - Clicking the header collapses or expands the section, and the file tree takes
    the space.
  - Collapsed, Kernels shows how many are running, and Resources names anything
    at 80% or more with a red dot ("GPU high").
- Kernel rows, **11px**:
  - Each row shows the language icon (R, Python, Nextflow), the version (a
    pipeline job shows its progress, "14/22"), what is attached, memory, and a
    trash button on hover.
  - No kernel name, so "R" and "Python" do not appear.
  - Clicking a row opens the same details pop-up as the kernel pickers, to the
    right of the row. A pipeline job's pop-up shows its progress, run time,
    executor and work directory, with Open the log and Show in Jobs.
  - The trash button turns the row into an inline "Shut down R 4.4.1? Cancel /
    Shut down" confirmation.
- Resources, **11px**, in a 2 × 2 grid (CPU and MEM, then GPU and Disk). Each item
  shows an icon, the label and a bar of the same length for every item, with no
  percent. Items are 22px high with 8px side padding, so the content sits centred
  in the hover shading. Clicking an item opens a details popover with the percent.
- Inside the pane, every row's hover shading starts 6px from the pane's left edge
  and ends 6px from the window's right edge. Its first icon or chevron starts
  14px from the pane's edge (file tree, section headers, kernel rows, resources),
  and so does the project name. Files inside a folder line up under the folder's
  icon.
- Pop-ups from this pane (resource details, kernel details) open to the left of
  the row.

## Editor
- The document bar has one kernel picker per language. It shows only the language
  icon (R, Python) and the version, for example the R icon followed by "4.4.1",
  with no status dot, language name, kernel or session ID. The icon is 14px and
  the picker's left padding is 3px, so the icon sits evenly inside the rounded end.
- Clicking a picker opens its drop-down: language, version and state, then the
  kernel, library or environment, machine, the documents using it and memory, then
  the session list (the current session's ID, New session, and another installed
  version to switch to). Session IDs appear only there.
- Run all shows no keyboard shortcut; ⌥⌘R is in its tooltip.
- The Data Explorer uses the UI font, with tabular digits, and shows column names
  exactly as written (no capitals).

## Console
- The session switch at the right of the Console's tab strip shows each session
  (R, Python) as its language icon and version, like the kernel pickers, with no
  status dot or language name. Clicking one shows that session's console; the
  tooltip gives the language, version and state.

## Agents pane: a chat, after Posit Assistant (draft 18)
Draft 18 replaces the terminal tabs with one chat pane laid out like Posit
Assistant in Positron, in Sandhill's own fonts, colours and glass. The plan behind
it is `agents-v2-plan.md` in the project's design folder. Claude Code and Codex
both use it, still signed in with your own plans, and so do open models through
omp.

- **Header** (the pane's tab strip): the conversation's provider icon and title,
  then + (new conversation), a clock (history) and a gear (conversation
  settings), then minimize and maximize. The clock shows an orange dot when
  another conversation is waiting for you.
- **Your messages** are grey rounded bubbles on the right. Under each, in 11px
  grey with an eye icon, is what went with it: the session (for example
  "R 4.4.1 · 14 variables · analysis.qmd › markers").
- **Replies** are plain text, without a bubble. Inline code, gene names and
  file paths are small grey pills in the code font. File names are orange links
  that open the file.
- **Thinking rows**, "Thought for 3s ›", are 12px grey and collapsed; opening one
  shows the reasoning with a grey rule on its left. Codex sends these. Claude
  Code's hooks don't carry its thinking, so a Claude conversation shows only a
  "Thinking…" row with a spinner while it works.
- **Step rows**, one line each at 12px:
  - a status icon: a check when done, a red x when it failed, a grey x when you
    denied it, a spinner while it runs;
  - the tool in bold code font (`run_code`, `run_cell`, `read`, `edit_cell`,
    `add_cell`), then the agent's short description;
  - a detail on the right: "R · 1.2 s · 1 plot", "lines 41–44 of 120",
    "+2 −0";
  - a › chevron. Opening a row shows the code, its output and any plot in one
    card. The card's header names where it ran ("Console · R 4.4.1 · 1.2 s"),
    and its foot has **Run again**, **Copy** and **Insert into file**. Edits open
    to their diff; reads to the lines read.
  - `run_code` runs in the Console, marked as the agent's. `run_cell` runs a cell
    of the linked file as if you pressed Run. `add_cell` writes a new cell below
    your active cell.
- **Approval card**: a step that needs you waits where it will run, with an
  orange edge. It names the action ("Add a cell to analysis.qmd?"), where it goes
  and the session that runs it, and shows the code as a diff. The buttons are
  **Allow once**, **For this chat**, **For this project** and **Deny**. Allow
  adds the cell; denying leaves the step crossed out (grey x) and the agent asks
  what to do instead. While a card waits, the status bar says the agent is waiting
  for you.
- **Turn footer**, after each finished reply: a copy button, the time and the
  turn's tokens, in 11px grey. No cost is shown, because plan sign-ins aren't
  billed per message.
- **Message box**: rounded, with a paperclip on the left (files and images can
  also be pasted or dragged in), "Ask Claude…" / "Ask Codex…" as the
  placeholder, and a round orange send button that becomes a stop button while
  the agent works. The grip on its top edge resizes it. Its bottom row:
  - left: web search (globe), session context on or off (eye; off shows a
    crossed-out eye) and the mode (hand). Any mode other than Normal shows its
    icon and name in orange.
  - right: the linked file with its language icon (opens the file; code goes into
    it), the context ring and tokens used (opens the context breakdown above
    the pointer), and the model with its effort, "Opus (High) ⌄" (opens the
    model list and the effort knob from draft 17). The effort bar is 16px
    tall and the brain knob 30px, so the knob stands out above and below the bar.
    The knob's centre stops 15px from each end, so the knob stays inside the
    pop-up. The bar is inset 4px at each end, and its filled part ends exactly
    under the knob's centre.
- **Mode picker** (the hand): Normal (safe steps run, risky ones ask), Auto (a
  second model reviews risky steps), YOLO (everything runs except steps marked
  dangerous; for Claude this restarts it on the same conversation), Restricted
  (asks before every step), Ask (read-only: no edits, no code runs) and Plan
  (writes a plan, then asks before carrying it out). Under them, "Agent code
  goes": **In the file** (the default) or **In the Console**.
- **Gear menu**: Show terminal (the agent's own terminal, for the rare screen
  the chat can't show yet, with "Back to chat"), Export as Markdown, HTML or
  JSON, Write a report (`/report`), Write a notebook (`/notebook`), Rename and
  Delete.
- **History** (the clock) replaces the chat with a list of conversations
  grouped as Needs you, Running, Today, Yesterday and Earlier, with a search box
  at the top. Each row shows the provider icon (with a dot when waiting or busy),
  the title, and the linked file, model and time in 11px. Rename and delete
  appear on hover. Opening a conversation also brings its linked file to the
  front of the editor.
- **New conversation** (+): pick Claude Code, Codex or omp (each button shows
  only the provider icon and name), see which file it links to (the active
  editor file), then type in the message box. **Cancel** (next to the "New
  conversation" heading, or the x on its tab, or esc) goes back to the
  conversation you were in.
- **Provider icons** are letter chips, not company logos (see "Icons and
  licences").

## Session pane (left)
- Variables can be selected and deleted to free memory; Sandhill runs `rm()`
  and `gc()` in that session.
- The session pickers in Variables and Packages are the same as the editor's kernel
  pickers: language icon and version, with the same drop-down.
- Help searches by package.
- Git shows a commit graph with avatars, then staged and unstaged files with a
  commit box. Everything below its tab strip is **11px** and never bold.
- Plots has a zoom dialog: set the size or pick a preset, then copy, or save as PNG, PDF
  or SVG. Plots stay white in dark mode.

## Status bar (26px)
- Left: the connection, then the kernel state ("R idle", "Python running") and
  jobs of the machine picked in the title bar's tabs. The connection icon starts
  14px from the window's left edge.
- The connection item is the machine's icon and name: a cloud and "gpu01", or a
  screen and "Local". The icon is the same one as on the title bar's tabs, and
  it shows the state. There is no status dot:
  - connected: the plain icon;
  - reconnecting: the icon in orange, pulsing (steady orange with reduced motion);
  - disconnected: the icon in red with a slash through it, and the name in red.
    The kernel state and jobs are hidden until it reconnects.
  The machine's tab in the title bar shows the same state.
- Its tooltip reads "gpu01 · sandhilld 0.3.0 · connected".
- Clicking it opens a pop-up above it, at 11px like the other pop-ups:
  - A server shows: name and state, then address, how it connects (ssh and the
    user), the sandhilld version and how it runs, time connected, latency and
    kernels. Its actions are Reconnect, Disconnect (or Connect when
    disconnected) and Open the sandhilld log.
  - Local shows "This Mac", state "running", the sandhilld version (launchd
    agent), how long it has run and kernels, with Restart sandhilld and Open the
    sandhilld log.
- Right: "agent is waiting" notice, then 5-hour and weekly plan usage for
  Claude Code and Codex, then cursor position.
- Each usage figure is a **14px ring** (2.5px stroke) that fills clockwise from
  the top, followed by the percent and the time until it resets. The ring is grey,
  and red from 80%. The usage popup that opens on click keeps its bars.

## Type and colour
- 13px for all UI text, in the macOS system font; 13px is the standard text size in
  macOS apps.
- Maple Mono NF (the Nerd Font build of Maple Mono, version 7.8) at 13px for all
  monospace text: the code editor and its outputs, Variables, the console and
  terminal, the agent's terminal, and code in Help, Git and the Agents chat. The Data Explorer is the
  exception: it uses the UI font. The font's Nerd Font icons let shell prompts and
  command-line tools show their symbols. Maple Mono is under the SIL Open Font
  License; the mockup carries its regular, italic, medium, semibold and bold styles.
  There is no other monospace font.
- Code ligatures are on (Maple Mono's default): `<-` shows as an arrow, `|>` as a
  triangle, and so on. The characters in the file are unchanged.
- The exceptions are the kernel and resource rows and the Git pane's contents at
  11px. In the Agents chat, step rows, thinking rows, code cards and the chips
  under the message box are 12px, and the context line under your messages, turn
  footers and history details are 11px.
- **Every popup and drop-down is 11px** (icons 13px): the context breakdown,
  model and effort, mode, gear menu, kernel and session drop-downs, usage,
  command palette, context menus and the plot zoom dialog. The context
  breakdown is 340px wide and the model popup 268px, aligned to the right edge
  of the pane.
- Accent: `#c2570c` in light mode, `#f29a45` in dark mode.

## Icons and licences
Every icon is drawn for Sandhill. No company's logo appears, because logos are
trademarks and their owners must approve each use in a product that is sold.
Product names in plain text ("Claude Code", "Codex", "Opus 5.5", "Python") are
fine: they only say what Sandhill works with.
- **Chips:** a 14 × 14 rounded square (radius 4) inside a 16px box, with one or
  two bold letters in the system font.
- **Language chips:**
  - R: "R", white on blue `#2368b8`;
  - Python: "Py", dark blue `#22466e` on yellow `#f2c53d` (not the Python
    two-snake logo);
  - Nextflow: "nf", white on green `#1fa784`.
- **Agent chips:**
  - Claude Code: "CC", white on brown `#9a5b41` (not Claude's spark);
  - Codex: "Cx", white on graphite `#3d4552` (not OpenAI's knot);
  - omp: "π", white on violet `#6b4fc8` (not omp's own mark).
- **File icons:**
  - `.qmd` shows a "Q" (not a mark like Quarto's).
  - `CLAUDE.md` and `AGENTS.md` show a plain four-point sparkle on coral.
- **The app icon** is Huan's own painting.
- **Fonts:**
  - The system fonts need no licence in the app, because the app does not ship
    them.
  - A font the app ships must carry its licence text in the app's third-party
    notices.
  - Maple Mono NF is under the SIL Open Font License. Its Nerd Font symbols come
    from sets under other licences:
    - Font Awesome and Codicons are CC BY 4.0, which needs a credit.
    - Material Design Icons are Apache 2.0.
    - Others are MIT.
    - One set, Font Logos, has no licence.

## Open questions
- The monospace font for a commercial release: Maple Mono NF (needs third-party
  notices), plain Maple Mono (only its OFL text), or the system's SF Mono (nothing
  to ship, but no ligatures and no Nerd Font symbols).
- Where banked plan-usage resets come from.
- Whether more panes should be movable by dragging.
