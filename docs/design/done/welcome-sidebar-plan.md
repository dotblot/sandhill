# Welcome screen as a macOS settings-style window

Status: approved by Huan on 2026-11-11 from a sketch modelled on a macOS
settings window (sidebar of sections, grouped content on the right), and
implemented the same day. Supersedes the single-panel Welcome with device
tabs (same day, earlier). While building: a not-connected device's only
action is the title's Connect (Reconnect after a failure); the window gained
a 720×480 minimum so the fixed sidebar and its sections always fit.

## Layout

Two columns filling the window under the existing title bar (traffic lights,
project capsules, theme switch stay where they are).

**Sidebar** (fixed width about 260px, the translucent side-pane glass, hairline
right edge):
- Section heading "Devices" (12px semibold, muted). One row per device:
  This Mac first, then saved servers alphabetically. A row is 34px: device
  icon (laptop or server), name (15px), status dot at the right end (green
  filled = connected, amber = connecting, red = failed, hollow muted = not
  connected). The selected row is filled with the brand accent and uses the
  primary text colour. Tooltip: full connection state or error.
- Last row of the section: "+ Connect to Server…" in muted text; opens the
  existing Connect to Server dialog.
- Section heading "Manage" with one row "Resources and Storage", which opens
  the resource manager for the selected device (as the old button did).
- At the bottom, centred: the crane app icon, "Sandhill 0.1.0" (name semibold,
  version muted, plus the build when present) and the short tagline.
- No "Recent" section (Huan).
- Right-click on a server row: Connect or Disconnect, Remove from List.

**Content** (solid editor surface, content column left-aligned with generous
padding, max width about 720px):
- Title: the device name, 24px bold, left-aligned like Finder (Huan). On the
  title's right: a "Connect" or "Disconnect" button for servers (none for
  This Mac).
- Subtitle (14px muted): state and counts, e.g. "Connected · 3 projects ·
  2 kernels running", or "Not connected", or the failure reason.
- For servers only, a row under the subtitle: "Connect automatically" with a
  small accent switch, default on, remembered per server in app settings.
  Off: opening Welcome does not connect to that server; it shows "Not
  connected" and the Connect button. It never affects work on the server.
- Projects as one grouped, rounded inset list (surface fill, hairline
  dividers inset past the icon). Row: Gruvbox folder icon, name (15px
  semibold) over its path (12px mono, muted), status at the right ("● 2
  kernels" or "opened 3 days ago"), and a pin star (always shown when
  pinned, on hover otherwise). Missing folders: dashed muted icon, muted name,
  amber "Folder not found". Pinned first, then by recency, as today. The list
  scrolls inside the content area when long.
- "Open Folder…" button right-aligned under the list; opens the existing
  folder dialog for the selected device.
- Not connected or failed: the list is replaced by a short explanation and a
  Connect / Reconnect button.
- Footer, muted 12.5px at the bottom of the content: "Kernels keep running
  after you close a project. Reopening it reattaches to them and restores
  your tabs."

## Behaviour rules

- Connection state is shown, never toggled: status dots plus Connect /
  Disconnect buttons. Connecting is asynchronous and may fail.
- Disconnect drops only Welcome's link to that device. Kernels, terminals,
  jobs and agents on the server keep running. When a project window on that
  device is open, Disconnect is disabled with a tooltip saying so.
- Connect automatically defaults to on, so existing behaviour is unchanged
  until a user turns it off. This Mac is always connected and has no switch.
- Controls keep the app's look: brand accent, glass, 13px popups, Lucide icons.
