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
  - Agents, a chat icon with an orange dot when an agent waits for you;
  - toggle right pane;
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
- One left edge for the left pane and the status bar: every row's hover shading
  starts 6px from the window's left edge, and its first icon or dot starts at 14px
  (file tree, kernel rows, resources, the status bar's connection dot). The
  Local | server tabs start at 6px and the project name at 14px. Files inside a
  folder line up under the folder's icon.

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
    model list and the effort knob from draft 17). The effort track is 16px
    tall, a little thinner than the 20px brain knob, and its filled part ends
    exactly under the knob's centre.
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
- **omp's icon** is its pi-like mark: a top bar with a short left leg and a
  longer right leg, filled with a pink to violet to light-blue gradient. Dark
  mode uses a slightly brighter gradient so it reads on the dark glass.

## Right pane
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
- Left: connection (`gpu01 · sandhilld`), R and Python kernel state, jobs.
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

## Open questions
- Where banked plan-usage resets come from.
- Whether more panes should be movable by dragging.
