# Plan: Welcome's Projects page, Pinned, and Resources in place

Status: approved by Huan on 2026-11-12 with every recommended choice:
Welcome opens on Projects; a project window's Resources and Storage goes to
this Welcome page; a pinned project that is open appears once, in Pinned,
with "Open" as its status. Builds on `welcome-projects-plan.md` (Open/Running
in the sidebar) and `welcome-sidebar-plan.md`.

Implemented the same day. Later the same day Welcome became its own window and opens on This Mac's page, with Projects between Devices and Manage (`idle-agents-welcome-window-plan.md`). The title-bar `+` still shows the current
device's page, where Open Folder… is. A category's items (paths, sizes and
why they are protected) open under its row. The page's ticks and choices are
dropped when another page is shown, unless an action is running.

Huan's request: Resources and Storage shows on Welcome's right side, not as
a dialog, and is laid out more clearly; the sidebar's Open and Running groups
move into one sidebar entry, Projects, whose page is on the right; Projects
also has a Pinned group, filled by the pin star on each device's project rows.

## Sidebar

```
Projects            ← one row; selected, the right side is the Projects page
Devices
  This Mac      ●
  ubuntu-lab    ●
  + Connect to Server…
Manage
  Resources and Storage
  Trusted Folders
```

Exactly one row is selected at a time, filled with the accent, as devices
are now. The Open and Running rows leave the sidebar.

## Projects page (right side)

Title "Projects", subtitle with counts ("2 open · 1 with kernels running ·
3 pinned"). Three grouped lists in the existing inset style (folder icon,
name over path, device icon and name, status on the right):

1. **Pinned**: projects pinned on any connected device, by recency.
2. **Open**: projects open in this window (click switches to it,
   right-click closes it, as now).
3. **Running**: not-open projects with live kernels on connected devices
   (click opens and reattaches).

Each project appears once, in the first group that has it; its status
still says the rest ("Open", "● 2 kernels"). Every row has the pin star
(shown when pinned, on hover otherwise), so pinning works here and on device
pages alike; pins stay per device in app settings, as today. An empty group
is hidden; with all three empty the page says how to pin or open one. A
muted line names servers that aren't connected, whose projects are not shown.

Pin on a device page keeps today's effect there (pinned first) and adds the
project to Projects → Pinned.

## Resources and Storage page (right side)

The same actions and safeguards as the dialog (`resource_manager.rs`): the
inventory, stop/force stop, cleaning chosen categories, uninstall, the typed
confirmation, per-device results. Only the layout changes.

- **Header**: title, one muted sentence ("Only the devices you tick are
  affected. Project folders are never removed."), then a row of device chips
  (tick boxes with the device icon and name; the current device ticked by
  default) and a **Read** button.
- **One card per ticked device**: its name and status at the top; errors in
  red right under it. Inside, three sections:
  - **Running**: rows with a kind icon (kernel, terminal, job, agent), label,
    project folder in muted text and state on the right. "Nothing running"
    when empty.
  - **Storage**: a thin bar split into removable / in use / protected with
    their sizes, then one row per category: name, path in muted text, size,
    and either a tick box (removable) or a muted "In use" / "Protected"
    label. The tick boxes replace today's separate category buttons.
  - **Projects**: one muted line, "N projects, never removed", expandable.
- **Actions**, at the bottom, each with its explanation beside it:
  - Stop background work (Force stop appears only after a stop that did not
    finish).
  - Clean ticked storage, with the total size ticked.
  - Uninstall Sandhill components, with its two options under it (also
    delete backend data; move this app to Trash afterwards).
  The confirmation replaces the action row in place, as now.

## Questions for Huan

1. Which page Welcome opens on: Projects (recommended), or the current
   device as now?
2. A project window's status-bar menu also opens Resources and Storage (now
   the same dialog). Should it go to this Welcome page (recommended, one
   design), or keep the dialog there?
3. A project both pinned and open: once, in Pinned, with "Open" as its
   status (recommended), or in both groups?
