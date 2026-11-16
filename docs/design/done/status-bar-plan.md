# Redesigning the status bar

Huan, 2026-11-15: "redesign the status bar". Status: approved and built
the same day, with all three answers yes and one addition: the resources
can be hidden and shown again, and while hidden they are not measured.

## What is there now (`view.rs::render_status`)

Left to right: the device (`connection.rs`), a Restricted chip
(`welcome/trust.rs`), four resource meters (`panes/machine.rs`), a running
compression or extraction (`shell/archive_tasks.rs`), the latest problem or
notice, and at the far right each agent's limits (`agents/quota.rs`).

| Piece | Shape now | Problem |
|---|---|---|
| Device | 6px box, 20px tall, laptop icon, name, dot | Not the capsule shape the title bar and tabs use |
| Restricted | Round pill with an amber outline | A third shape |
| CPU, memory, GPU, disk | 13px icon and a 14px ring, no number | Can't be read without clicking; the four icons look alike at that size |
| Archive task | Text, bar, %, a toolbar button | Its own shapes again |
| Problem or notice | Text in the middle of the bar, cut off | No way to read a long error; it moves when meters appear |
| Agent limits | 6px box, 24px tall, agent mark, two 22px rings labelled "5h"/"7d" in 8px text | Labels too small to read; taller than the rest |

Four heights (18, 20, 24 and the bar's 26px) and four shapes in one bar.

## The new bar

Three places, each with one job, all in the round capsules the title bar
uses (`look::capsule`, 20px tall, neutral wash, no edge):

- **Left, where you are:** one capsule with the device (icon, name, dot)
  and, after a thin divider, its resources as numbers: `CPU 12%`,
  `Mem 63%`, `GPU 4%`, `Disk 71%` (red at 80% and over, as now). The
  device opens the connection menu and each number its details, as now.
  Restricted Mode is an amber capsule beside it (lock and "Restricted").
- **Middle, what is happening:** a running compression or extraction as
  a capsule (name, a thin bar, %, and a × to cancel), then the latest
  message: notices grey, problems red with a × to dismiss. A long message
  shows in full when hovered.
- **Right, AI limits:** one capsule per agent with what is left, as
  numbers coloured green, amber or red (the panel's own colours):
  `✳ 5h 62%  7d 88%`. Not yet checked shows `–`. Clicking opens the
  panel, as now.

When the window is narrow, the message shortens first, then the resource
numbers hide (their capsule keeps the device), then the agents' numbers
hide (their marks stay).

The resources capsule ends with a chevron that hides them (Huan's
addition). Hidden, the daemon is no longer asked for CPU, memory, GPU and
disk, so the Kernels pane's per-kernel memory and the 80% memory warning
pause too; the choice is this Mac's, for every device, and kept. Shown
again, they are measured at once.

Text is 13px everywhere in the bar, icons 12px, capsules 20px, 8px between
capsules.

## Not changed

What each piece does when clicked, its popups, what is checked and when
(limits are never asked for by themselves), and the order: device and
resources at the left, limits at the right (decisions in `window.md` and
`agents.md`).

## Questions for Huan

1. Resources as numbers (`CPU 12%`) instead of rings: agreed? It is wider
   (about 260px instead of 160px) but readable at a glance.
2. Limits as numbers (`5h 62% · 7d 88%`) instead of rings: agreed?
3. Problems in the bar get a × to dismiss them (today one stays until some
   later action, such as reconnecting or opening a file, clears it).
   Agreed?
