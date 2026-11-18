# Window

Settled decisions about the window, its panes, the title bar and Welcome.
Moved here unchanged from `CLAUDE.md` on 2026-11-15; where an older plan
differs, this wins.

## Layout and pane sizes
- Window layout (Huan, 2026-11-02): three columns, 1:2:1. Left: Variables,
  Help, Packages, Kernels in one group; Files, Git, Plots, Viewer in the
  other (the two groups split 1:1). Middle: the editor, with Console,
  Terminal and Jobs under it. Right: the AI pane. Resource indicators follow
  the device connection at the left of the status bar (Huan, 2026-11-08).
  The status bar (Huan, 2026-11-15; `docs/design/done/status-bar-plan.md`)
  is 20px round capsules with 13px text: the device with its resources as
  numbers ("CPU 12%", red at 80%), then Restricted in amber, a running
  task with × to cancel, the latest message (problems red with × to
  dismiss, full text on hover), and at the right each agent's limits as
  what is left ("5h 62%  7d 88%", green/amber/red, "–" until checked). A
  chevron hides the resources, and hidden they are not measured at all
  (this Mac's choice, for every device). Narrow windows drop the resource
  numbers, then the agents' numbers.
  The device popup (Huan, 2026-11-16): the device, its state and one grey
  line (sandhilld's version, how long it has run, for another computer its
  address), its usage as rings (a click opens that resource's details),
  its running kernels with the files using them, a "Show usage in status
  bar" switch, then the connection's actions and Sandhill's version. The
  switch is on by default; off, the status bar has neither the numbers nor
  the chevron, and usage is measured only while the popup is open.
  The AI pane and the right side pane are shown by default. Each pane's tab
  bar ends with a maximize button: the pane fills the space between the title
  and status bars until clicked again or Escape. A title-bar toggle between
  the left and AI toggles hides the middle column (editor with Console,
  Terminal and Jobs); the AI pane takes its width, or the left column when the
  AI pane is hidden; one column always stays, and opening or running a
  document shows the middle again (Huan, 2026-11-13). Pane height and size
  icons (Huan, 2026-11-13, redrawn 2026-11-14 after his RStudio-like
  references): a short window (title row only) collapses a pane to its
  title row, a tall window with a title bar fills its column, and two
  overlapping windows restore the split, so the two buttons never match;
  corners out/in
  maximize to and restore from the window. The left and middle toggles keep
  plain outline icons. The AI pane's header shows the conversation's name as
  plain text, not a tab; AI replies colour code blocks in Python, R, shell,
  JSON, YAML, TOML, Markdown, SQL, diff, HTML, CSS and JavaScript.

## Title bar, windows and Welcome
- Title bar (Huan, 2026-11-06): one capsule per project, including multiple
  projects on the same device. The active capsule shows the project name,
  a divider and open file tabs; inactive ones show the name and file icons.
  Each project name has a local-device or remote-server icon. File labels
  are not repeated in the editor pane. Left panel controls and the right
  theme switch have no enclosing capsule. The left status bar shows the
  active project's device and connection state. Windows (Huan, 2026-11-12):
  exactly one maximized main window holds the projects as capsules; Welcome
  is its own window, as wide as its sidebar and widest page (1036px; Huan,
  2026-11-13, after half the display left empty space beside the lists) and
  half the display's height, at least 800×600 and never resized smaller,
  never drawn inside the main window. Sandhill
  starts with the projects open at the last quit, else with Welcome alone;
  `+` brings Welcome up beside the main window, as does the house button
  beside the theme switch (Huan, 2026-11-13), and the project chosen opens
  there, closing Welcome; closing the main window (or its last project)
  brings Welcome back; closing Welcome alone quits (with the background-work
  question). Welcome's sidebar and content reach the window top; the sidebar
  is Devices, Projects, then Manage (General, Resources and Storage,
  Trusted Folders, Agents), one row selected with its page on the right;
  it opens on This Mac's page. A device has one name everywhere (Huan,
  2026-11-15): the one given it, else "This Mac" ("This computer" off a
  Mac) or the server's ssh name, in Welcome, the status bar, title-bar
  capsules, cards and menus. Projects lists Pinned (the star on any device's projects),
  Open, then Running (live kernels or agents, not open), each project once.
  Rows show live kernels and agent conversations. Devices can be renamed and
  projects given a built-in folder/file icon; both are this Mac's app
  settings, never written to the device or folder. Resources and Storage is
  a Welcome page, also from a project window's device menu, never a dialog.
  Project rows (Huan, 2026-11-14) have a ⋯ menu (also on right-click): Save
  All Files, Stop All Kernels (confirmed), Close Project, Change Icon.
  Closing a project with unsaved files or running kernels asks once: Save
  and Close (default, kernels keep running), Save, Stop Kernels and Close,
  or Cancel. Removing an open project from the list asks to close it first.
  Choosing folders (Huan, 2026-11-14): on this Mac, the system's folder
  window; folders dragged from Finder onto Welcome open as projects here.
  Servers keep Sandhill's own chooser, read through the daemon, with
  places, a clickable path, filter/typed paths, keyboard, New Folder,
  hidden-name toggle, faded files and Git/Sandhill marks. Files are chosen
  the same way (Huan, 2026-11-18): Variables' Import asks for its dataset
  in the system's window on this Mac and in Sandhill's chooser, in its file
  mode, on servers, instead of a file list of its own.
- Settings (Huan, 2026-11-15): Welcome's Manage section starts with
  General, this Mac's choices for every project: the theme (System, Light,
  Dark), notebooks' runs in the Console and plots in Plots, suggesting the
  next cell, following the AI's edits, wrapping long lines per kind of
  file, the Files sort order, and whether plot windows' settings column and
  the Data Explorer's column panel show at first. ⌘, and Sandhill →
  Settings… open it; there is no separate settings window. Each choice
  also stays beside what it changes, and changing it anywhere applies to
  every open project at once. Project settings stay per project, in a
  card drawn the same way (Huan, 2026-11-15): Kernels, Running and AI
  groups, the round kernel picker with a floating list, capsule tabs for
  two-way choices, and a Save button, as it writes a file collaborators
  may share.
- Linux (Huan, 2026-11-18): Sandhill comes to Linux by way of Ubuntu
  (`docs/design/linux-plan.md`); other Linux systems and desktops wait.
  There Sandhill draws close, minimise and maximise in its title bar, on
  the right as Ubuntu puts them, and all three always show, whatever the
  desktop's own button setting says.
