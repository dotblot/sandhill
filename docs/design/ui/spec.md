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
- The file tree has guide lines, and folders open and close (restyled in draft 26).
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
- The two side panes swapped sides (undone in draft 22).

Draft 22 (2026-10-19) changed:
- The title bar is 26px, as tall as the status bar.
- The title bar's tabs sit 6px below the window's top edge.

Draft 23 (2026-10-19) changed:
- The Agents tab shows the linked file (language icon and name) in place of the
  title the agent wrote. The file no longer shows in the message box.
- No text is smaller than 10px, the smallest size macOS uses.

Draft 24 (2026-10-19): conversations belong to the project, and context follows
you. A project has as many conversations as you start. The file a conversation
works in follows the editor's active tab unless you pin one, and @ in the
message box adds files, chunks, variables or plots.

Draft 25 (2026-10-20) changed:
- Clicking the project name at the top of the left pane picks another project on
  the same machine. The name has a folder icon in front.
- The machine tabs always take you back from the Start screen.
- The Start screen's projects open on their machine.

Draft 26 (2026-10-21) changed:
- The title bar's tabs and toolbar icons sit in capsules (fully rounded glass
  bars), as in Safari on macOS 26. The traffic lights stay as they are.
- The file tree uses one column for chevrons and one for icons, grey glyph icons,
  and plain indent guides, as in VS Code.

Draft 27 (2026-10-21) changed:
- File and folder icons are coloured glyphs, the same in every pane.
- Every pane's tabs sit in a capsule, like the title bar's.
- Variables has an Import button for datasets.
- The left pane has a sort button: by name, size or date modified.
- A new edit capsule in the title bar: undo, redo, cut, copy, paste, save and
  save all.
- The side panes are back: files on the left, Variables / Plots on the right.
- Every pane and the status bar show only the machine picked in the title bar,
  so local and remote work never mix; see "One machine at a time" below.

The clickable mockup is [sandhill-layout.html](sandhill-layout.html), also
published at https://claude.ai/artifact/GVLbkapTsw8FP55ie3RESs (draft 16 stays
at https://claude.ai/artifact/DSC3kDKpeh3XhX8ogsiVA9). It has light and dark
themes and four screens: Workspace, Data Explorer, With agent and Start.
The screenshots are `workspace-light.png`, `workspace-dark.png`,
`with-agent-light.png`, `with-agent-dark.png`, `agents-history-light.png` and
`agents-history-dark.png`, plus `workspace-local-light.png` with the Local tab
picked. They show a 1452 × 860 point
window at 2× (2904 × 1720 pixels), tagged 144 dpi so they open at true size on a Mac.

The mockup is the reference. Where this spec and the mockup differ, the mockup wins.

## Window and panes
- Panes are floating, rounded, translucent glass cards on a warm gradient
  window background, in the style of macOS Liquid Glass. The left pane is not a
  card: its content sits directly on the background.
- Top to bottom: the title bar (26px), the panes, the status bar (26px).
- Pane order from left to right:
  - left pane: file tree, Kernels and Resources;
  - Agents, which only shows when toggled on;
  - the editor column, with the editor above and the Console below;
  - the right pane, with two groups: Variables / Help / Packages / Git on top,
    and Plots / Viewer below.
- The gap is **3px**: between panes, from the panes to the title bar and to the
  right window edge, and to the left edge when the left pane is hidden. The left
  pane reaches the left edge. The panes sit directly on the status bar, with no
  gap below them.
- Gaps between panes are drag handles: dragging one resizes the panes on
  either side, and an orange line shows on hover. Console, Variables and Plots can be
  dragged by their tab strip to under the editor or into the right pane.
- Every pane has minimize and maximize buttons at the right end of its tab
  strip:
  - Minimized, only the tabs show and the contents are hidden.
  - Maximized, the pane fills the window, opaque, until restored.
- **One corner radius, 12px**, for every rounded thing in the window: panes,
  tabs, kernel rows, inputs, buttons and popovers.
- **Pane tabs sit in capsules**, like the title bar's: the editor's file tabs, the
  Console's tabs and its R / Python switch, Variables / Help / Packages / Git,
  Plots / Viewer, and the Agents tab.
  - The capsule is 22px high and fully rounded, and as wide as its tabs.
  - Each tab is an 18px pill.
  - The active tab is a raised pill: the pane background colour (white at 13% in
    dark mode), a hairline edge and a soft shadow. Hover uses 6%.
  - Close buttons and counts stay inside the tab.
  - Minimize, maximize and the pane's own buttons stay outside the capsule.

## Title bar (26px, as tall as the status bar)
- The title bar is transparent, on the window background, and dragging its empty
  space moves the window. From left to right:
  - the traffic lights, plain, outside any capsule;
  - the **tabs capsule**, which fills the width: one tab per machine, then a thin
    divider and + (Connect to server, ⇧⌘O), which opens the Start screen;
  - the **edit capsule**:
    - undo (⌘Z) and redo (⇧⌘Z);
    - cut (⌘X), copy (⌘C) and paste (⌘V);
    - save (⌘S) and save all open documents (⌥⌘S), each group divided.
    - Buttons that can't act are dimmed: redo until you undo, save when the
      active document has no unsaved changes, save all when nothing is unsaved.
    - Saving clears a tab's unsaved dot.
    - The tooltips name what they save.
  - the **toolbar capsule**: the toolbar icons.
- **Capsules** are 20px high and fully rounded (the one exception to the 12px
  corner rule), and start 6px below the window's top edge.
  - Light mode: a light grey tint (the text colour at 6% over the pane background
    at 40%) with a hairline edge and a soft
    shadow, over a blurred backdrop.
  - Dark mode: white at 6% with a white hairline at 10%.
  - Inside, a 1px divider (12px high) separates groups.
- **Machine tabs** follow the style of browser tabs in a macOS title bar:
  - On the Start screen no tab is picked. Clicking one goes back to that
    machine's open project.
  - Tabs share the capsule's width equally and centre their content. Each is a
    16px-high pill, 2px inside the capsule. The capsules sit 3px above the
    panes, and the traffic lights are centred on the same line.
  - Each tab shows the machine's icon (13px), the machine's name (500 weight)
    and the project's path in the faint colour, cut short with "…".
  - An orange dot after the path means an agent on that machine is waiting for
    you, so you see it from the other machine's tab.
  - The picked tab is a raised pill inside the capsule: the pane background colour
    in light mode (white at 13% in dark mode), a hairline edge and a soft shadow.
    The others are flat, with 6% shading on hover and a thin divider between two
    flat tabs.
  - The icon shows the connection state, like the status bar's connection item:
    plain when connected, orange when reconnecting, red with a slash when
    disconnected.
  - Clicking a tab switches the whole window to that machine (see "One machine
    at a time").
- **Toolbar icons**, in the capsule at the right end of the title bar. Each is a
  26 × 16px round button with a 15px icon. A divider separates the pane toggles
  from search and the menu:
  - toggle the left pane (files, Kernels, Resources), ⌘B;
  - Agents, a chat icon with an orange dot when an agent on this machine waits
    for you;
  - toggle the right pane (Variables, Plots), ⌥⌘B;
  - search, which opens a ⌘K palette for files, symbols, commands and "search in files";
  - menu, with Connect to server, Open project, New project, Recent projects, Show console and Settings.
    It opens under its button and stays inside the window.
- There is no settings gear elsewhere: Settings lives in the menu.

## One machine at a time
Everything in the window belongs to the machine picked in the title bar, so local
and remote work are never shown together.
- **Each machine has its own:**
  - project and file tree;
  - editor tabs and documents;
  - kernels, Console sessions, Terminal, Jobs and Problems;
  - Variables, Packages, Help (only that machine's installed packages) and Git;
  - Plots and Viewer;
  - Agents conversations;
  - ⌘K palette;
  - status bar items: connection, kernel state, jobs, the "agent is waiting"
    notice and cursor position.
- **Each project on a machine is kept apart the same way**: see "Project name"
  under Left pane.
- **A project with nothing open yet** says so in each pane:
  - the editor: "No files open in …";
  - the Console: "No R or Python session in …", with Start R and Start Python
    buttons;
  - Variables, Packages, Git and Plots: what will show there;
  - Agents: no conversations.
  - Its file tree, the Terminal and the ⌘K palette (its files) work as usual.
- **Switching remembers where you were.** Each machine keeps its open tab,
  Console session, plot, conversation and Help search, so switching back returns
  to the same place.
- **Shared by both:** the plan usage rings in the status bar, because they belong
  to your Claude and OpenAI accounts, not to a machine.
- **The mockup's Local example:**
  - the `thesis-figures` project, with `fig3.qmd` (a volcano plot) and `theme.R`;
  - one R 4.4.1 session and no Python;
  - its own Git history;
  - two plots;
  - one past conversation.

## Left pane (260px, minimum 236px)
- It shows the machine picked in the title bar's tabs.
- **Project name** at the top: a folder icon (15px), the name (semibold) and a
  small ⌄, as one button with 6% shading on hover.
  - Clicking it opens "Projects on gpu01" (or "on this Mac"): each project with
    its folder icon, name and path, a check on the open one, and when the others
    were last used.
  - Then Open project… (⌘O) and New project… (⇧⌘N), which go to the Start
    screen.
  - Picking a project switches the whole window to it, like switching machine.
    The title bar's tab then shows that project's path.
- Project name row. At the right:
  - + (new file in the project's top folder);
  - **sort**: up and down arrows, which open "Sort files by" Name (A to Z),
    Size (largest first) or Date modified (newest first), with a check on the
    current one. Folders always come first, and a folder's size is the sum of
    its files.
  - refresh.
  Then the file tree. Each row's tooltip gives its name, size and when it was
  modified.
- The file tree, in the style of VS Code:
  - Every row has a 14px **chevron column**, then a 16px **icon column**, then
    the name. Files leave the chevron column empty, so a file's icon lines up
    with the folder icons at the same level.
  - Each level is indented 16px, so a folder's children have their icons one
    step right of the folder's icon.
  - **Indent guides**: a 1px vertical line (the text colour at 24%) runs through
    the centre of each open folder's chevron column, beside all its children.
    There are no elbows.
  - Clicking a folder opens or closes it, and its chevron turns to point down
    when open.
  - **Icons are coloured glyphs**, all drawn for Sandhill, and **the same in every
    pane**: the file tree, editor tabs, Git, the @ menu, History and the Start
    screen. Colours come from the muted palette:
    - R, README and Python: blue;
    - Quarto: slate;
    - notebooks and .gitignore: rust;
    - tables and shell scripts: sage;
    - data files: teal;
    - lock files and LICENSE: mustard;
    - YAML, images and CLAUDE.md: coral;
    - Markdown: mauve;
    - other files: sand.
    Dimmed files show their icon at 55% opacity.
    - Folders are filled: data teal, results mustard, figures coral, tests
      mauve, .sandhill sand, and other folders sage.
    - Folders are filled. Some have a cut-out emblem: data (a cylinder), results
      (bars), figures (a line chart), tests (a flask), .sandhill (a ring).
    - R files show "R", Quarto files "Q", R Markdown "Rm", Python "Py", and
      notebooks an orbit.
    - Tables show a grid, and data files (.rds, .h5ad, .h5, .gz, .mtx, .bed) a
      cylinder.
    - Lock and config files show braces, and images and PDFs a picture.
    - Shell scripts show a prompt, and .bib files a book.
    - README shows an info circle, CLAUDE.md and AGENTS.md a sparkle, LICENSE a
      badge, and .gitignore a branch.
    - Anything else shows a page.
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
- One left edge for the left pane and the status bar: every row's hover shading
  starts 6px from the window's left edge, and its first icon or chevron starts at
  14px (the file tree's chevron column, section headers, kernel rows, resources,
  the status bar's connection icon). The project name's folder icon starts at
  14px. Top-level file and folder icons start at 32px.
- Pop-ups from this pane (resource details, kernel details) open to the right of
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

- **Conversations belong to the project** on its machine. Start as many as you
  like; each has its own context, so unrelated tasks don't fill one context
  window, and two agents can work at once.
- **Header** (the pane's tab strip):
  - The conversation's provider chip, then its **focus file**: the language icon
    (14px), the name and a small ⌄. The tab shows no title written by the agent.
  - The agent sees the focus file, and the code it writes goes into it, below
    your active cell.
  - By default the focus file **follows the editor's active tab**.
  - While the agent is working, it keeps the file it started in.
  - Clicking it opens a menu: Open the file, then "The agent works in": **Follow
    the editor** (with the current file in grey), or **Pin to** one of the open
    files.
  - A pinned file shows a small orange pin after its name, and stays when you
    switch editor tabs.
- Then + (new conversation), a clock (history) and a gear (conversation
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
  also be pasted or dragged in), "Ask Claude… (@ to add files, chunks, variables
  or plots)" as the placeholder, and a round orange send button that becomes a stop button while
  the agent works. The grip on its top edge resizes it. Its bottom row:
  - left: @ (mention), web search (globe), session context on or off (eye; off shows a
    crossed-out eye) and the mode (hand). Any mode other than Normal shows its
    icon and name in orange.
  - right: the context ring and tokens used (opens the context breakdown above
    the pointer), and the model with its effort, "Opus (High) ⌄" (opens the
    model list and the effort knob from draft 17). The effort bar is 16px
    tall and the brain knob 30px, so the knob stands out above and below the bar.
    The knob's centre stops 15px from each end, so the knob stays inside the
    pop-up. The bar is inset 4px at each end, and its filled part ends exactly
    under the knob's centre.
- **@ mentions**:
  - Typing @, or clicking the @ button, opens a menu above the message box with
    the project's files, the open document's chunks, variables and plots, grouped
    under those headings and filtered as you type.
  - Enter or Tab picks the first match, and Esc closes the menu.
  - Each pick becomes a chip at the top of the message box (icon, name, ×), and
    goes with the message as extra context.
  - Unsent text and chips stay when the pane redraws.
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
  the title, and the file it last worked in, model and time in 11px. Rename and
  delete appear on hover. Opening a conversation also brings that file to the
  front of the editor.
- **New conversation** (+): pick Claude Code, Codex or omp (each button shows
  only the provider icon and name), then type in the message box. There is no
  file to choose: the note says it works in the file in front of you (named) and
  sees your live sessions, and that @ adds more. **Cancel** (next to the "New
  conversation" heading, or the x on its tab, or esc) goes back to the
  conversation you were in.
- **Provider icons** are letter chips, not company logos (see "Icons and
  licences").

## Right pane
- Variables can be selected and deleted to free memory; Sandhill runs `rm()`
  and `gc()` in that session.
- **Import**, next to the session picker in Variables, as in RStudio:
  - The menu, at 11px, is headed "Import a dataset into R 4.4.1". Each line shows
    the R function it uses:
    - From text (base) and From text (readr);
    - From Excel;
    - From SPSS, From SAS and From Stata;
    - From R data (.rds, .RData) and From 10x Genomics.
  - Each opens a preview with the code it will run. That dialog is not drawn yet.
- The session pickers in Variables and Packages are the same as the editor's kernel
  pickers: language icon and version, with the same drop-down.
- Help searches by package.
- Git shows a commit graph with avatars, then staged and unstaged files with a
  commit box. Everything below its tab strip is **11px** and never bold.
- Plots has a zoom dialog: set the size or pick a preset, then copy, or save as PNG, PDF
  or SVG. Plots stay white in dark mode.

## Status bar (26px)
- Left: the connection, then the kernel state ("R idle", "Python running") and
  jobs of the machine picked in the title bar's tabs.
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
- Nothing is smaller than **10px**, the smallest size macOS itself uses.
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
