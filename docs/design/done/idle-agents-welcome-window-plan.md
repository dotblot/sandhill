# Plan: idle agents stop, and Welcome as its own window

Status: approved by Huan on 2026-11-12 with every recommendation: projects
open at the last quit reopen in the main window at start (Welcome stays
closed until `+`); closing Welcome with no main window quits Sandhill (with
the keep-background-work question); there is exactly one main window, with
projects as capsules. The Welcome tagline is "Elegance in discovery." (done).

Implemented the same day. The daemon checks once a minute; activity is
tracked by each conversation's terminal host (input, commands, answers,
output, chat entries and reports), so it holds with no client connected.
Manual Stop is a stop button on a running conversation's History row.
Showing Projects or a device page reads the devices again. The Running group
also lists projects with agent conversations but no kernels. A project's
icon also shows in its title-bar capsule, after the device icon. Not tried
with a real 30-minute wait: the daemon test shortens the limit. On
2026-11-13 Huan changed Welcome's fixed 800×600 to half the width and height
of the whole display (menu bar and Dock included; the main window's display,
else the primary one), at least 800×600; it cannot be resized below 800×600.
Later that day Huan found half the width left empty space beside the
project lists, so Welcome opens as wide as its sidebar plus the widest page
(1036px, never beyond the display) and keeps half the display's height.

## 1. Idle conversations stop, and resume on the next message

Huan: a conversation idle for 30 minutes is stopped automatically and comes
back when a message is sent; History can also stop one by hand without
deleting it.

- **Who stops it**: the daemon, on the device where the agent runs, so it
  also happens after Sandhill quits with background work kept, and for every
  window watching that device.
- **Idle** means 30 minutes in which the conversation reports `idle` (not
  busy, not waiting for an approval or an answer) and gets no message, no
  terminal input and no output. A conversation working or waiting is never
  stopped, however long it takes.
- **Stopping** ends the agent's process the gentle way (its own quit, then
  a terminal hang-up), keeps the chat and the agent's session id, and marks
  the conversation "Stopped while idle" in the chat and History.
- **Resuming**: sending a message starts the agent again on its own session
  (as resuming an ended conversation does now), then sends the message.
  Antigravity has nothing resident except a small bridge; it is stopped the
  same way.
- **Agents that cannot resume** are never stopped automatically, and their
  manual Stop says the conversation cannot continue: Posit Assistant (its
  screen has no resume from the command line) and any conversation whose
  agent never reported a session id (for example one that ended before its
  first reply).
- **History**: each running conversation gets a "Stop" item in its menu
  (beside Rename, Delete); stopping keeps it in the list, greyed as stopped.
- **Setting**: the 30 minutes are fixed for now (no setting), as one
  constant in the daemon.

## 2. Welcome sidebar, device names, project icons, agent counts

- **Sidebar order**: Devices, then Projects, then Manage.
- **First page**: This Mac's page when Sandhill starts (replacing Projects
  first, decided earlier today).
- **Renaming a device**: right-click a device → Rename…, or double-click its
  name; the name is this Mac's label for it, kept in app settings. The ssh
  name stays in its tooltip and is what Sandhill connects to. The new name
  shows everywhere the device is named: Welcome, the title-bar capsules and
  the status bar. This Mac can be renamed too.
- **Running agents per project**: every project row (device pages and the
  Projects page) shows its running conversations beside the kernels, for
  example "● 2 kernels · 3 agents". The count is the project's running
  conversations on that device, read with the device's project list.
- **Open Folder…** moves under the list at the left, its icon aligned with
  the project icons above it.
- **Project icons**: right-click a project row → Change Icon…, which opens a
  small grid of the built-in Gruvbox folder and file icons (with a Default
  choice). The choice is this Mac's setting for that device and folder, not
  written into the project. It shows in Welcome's rows and the project's
  title-bar capsule.

## 3. Welcome as its own window

- **Starting Sandhill** opens Welcome alone, in its own 800×600 window, on
  This Mac's page.
- **Opening a project** from Welcome opens the main window maximized (the
  macOS zoom, filling the screen but not a full-screen space) with the
  project, and Welcome closes.
- **`+` in the main window** brings Welcome back in front of it, on the
  current device's page; the main window stays. The project chosen opens as
  another capsule in that main window, and Welcome closes.
- **Closing the main window** (its projects saved, kernels and agents kept as
  now) brings Welcome back.
- **Closing Welcome** while no main window is open quits Sandhill, with the
  usual question about keeping background work. With a main window open it
  just closes.
- The main window no longer shows Welcome inside it; the menu commands that
  showed it (Open Project ⌘O, New Project, Connect to Server, Manage
  Resources) bring up the Welcome window instead.

## Questions for Huan

1. When Sandhill starts and projects were open when it last quit: reopen
   them in the main window straight away (recommended; Welcome stays
   closed until `+`), or always start with Welcome only?
2. Closing Welcome with no main window open: quit Sandhill (recommended,
   with the keep-background-work question), or keep Sandhill running with
   no window, as many Mac apps do (the Dock icon brings Welcome back)?
3. Can more than one main window exist (for example ⌘N, or opening a
   project into a new window), or is there always exactly one
   (recommended: one, with projects as capsules)?
