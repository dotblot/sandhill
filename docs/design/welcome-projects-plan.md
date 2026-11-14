# Plan: Welcome sidebar with open and running projects

Status: approved by Huan on 2026-11-12 with A (projects as sidebar rows) and
capsules removed on Welcome only; implemented the same day. Builds on
`welcome-sidebar-plan.md`. Superseded the same day by
`welcome-projects-resources-plan.md`: Open and Running moved from the
sidebar into the Projects page.

Huan's request: on Welcome, the two columns reach the top of the window; the
title bar's project capsules go away there; every open project and every
project with running kernels appears in the sidebar, as two groups, across all
devices.

## Layout

- The sidebar and the content column start at the window's top edge, as in
  macOS System Settings. The traffic lights sit on the sidebar's glass; the
  theme switch stays at the top right, on the content. Dragging the empty top
  strip of either column still moves the window.
- Welcome draws no project capsules. Project windows keep theirs (recommended;
  see question 2).

## Sidebar, top to bottom

1. **Open** — projects open in this window, on any device, in capsule order.
   Row: folder icon, project name, and a small laptop/server icon with the
   device name in muted text; "● 2" when kernels run. Clicking switches to the
   project, as its capsule did. Right-click offers Close Project.
2. **Running** — projects that are not open but have kernels running, on any
   connected device. Row as above, with the kernel count. Clicking opens the
   project and reattaches its kernels.
3. **Devices** and **Manage**, unchanged.

Each group is hidden when empty. A project open and running is listed only
under Open. Projects come from devices Welcome is connected to; a server
with "Connect automatically" off is not asked until it is connected, and the
Running group says so in a muted line when such servers exist. The sidebar
scrolls when the groups are long; the app identity stays at the bottom.

## Unchanged

Device pages on the right, Open Folder…, the folder dialog, Connect to
Server…, Resources and Storage, and the 720×480 minimum window.

## Questions for Huan

1. Projects as rows directly in the sidebar (A, recommended: one click to go
   back, as with the capsules), or two sidebar entries "Open Projects" and
   "Running Kernels" that list projects on the right, grouped by device (B)?
2. Remove the capsules only on Welcome (recommended), or from project windows
   too? Without capsules, switching projects would always go through Welcome.
