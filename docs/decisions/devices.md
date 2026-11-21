# Devices, the daemon and background work

Settled decisions about the daemon, devices, quitting and closing, folder
trust and journaling. Moved here unchanged from `CLAUDE.md` on 2026-11-15;
where an older plan differs, this wins.

## The daemon
- The app reaches the daemon only through the wire protocol, by running
  `sandhilld attach --start` (later through ssh). It never opens notebooks
  or kernels itself, so local and remote work the same way.
- Daemon (`sandhilld`): Rust, tokio. Runs on Linux (systemd user service) and
  macOS (launchd agent). Kernels speak the Jupyter protocol (ipykernel for
  Python, Ark for R; IRkernel also works).
- Every kernel output is journaled by the daemon so clients can disconnect and
  replay. Never make output delivery depend on a client being connected.

## Quitting and closing
- Quitting defaults to keeping background work. The alternate choice stops all
  Sandhill kernels, terminals, jobs and agents on connected devices before
  stopping their daemons (Huan, 2026-11-08). Save edits first; failures keep the
  app open. Never force-kill remaining processes without a separate confirmation.
  Ordinary Disconnect/Reconnect and daemon upgrades retain independent processes.
  Closing a project alone still returns to Welcome. The quit question (Huan,
  2026-11-13) is a 380px card: the crane and "Quit Sandhill?" on one row,
  one sentence naming the connected devices, then full-width buttons stacked:
  Keep Running and Quit (default), Stop All and Quit, Cancel. A stop failure
  shows one red line, per-device reasons behind "Show details", and Force
  Stop and Quit in place of Stop All. No paragraph or keyboard hint.
  Closing a file (Huan, 2026-11-15; `docs/design/done/close-file-plan.md`)
  whose kernel or AI conversations still run shows a card in the same
  style: "Close x?", one sentence, a box to shut the kernel down (naming a
  busy kernel and its other files/Console) and one box per running
  conversation pinned to, given, working in or @-mentioning the file. All
  start unticked; stopped conversations stay resumable in history. Unsaved
  edits fold in (Save and Close, Don't Save, Cancel; else Close, Cancel).
  Files with nothing running ask only about unsaved edits, as before.

## Device management
- Device management lists resources, affected projects, storage paths/sizes and
  per-device results. Default to the current device only. Offline is not success.
  Cleanup/uninstall preserves history by default and project `.sandhill` data.
  Only checksum/provenance-owned components may be removed; never user Python/R,
  conda environments or standalone agent CLIs. Trash the current app last, only
  after selected devices succeeded. Unverifiable leftovers must be reported.
- What Sandhill installed (Huan, 2026-11-19; `docs/design/explore-next-plan.md`):
  Getting Ready writes each R package, IRkernel file and agent it installs
  into a record in the daemon's state folder (`installed-by-sandhill.tsv`).
  Remove What Sandhill Installed… (asked in its row) removes only those
  still at the recorded version and place; anything updated or reinstalled
  since is the user's and stays. This is the one exception to "never
  standalone agent CLIs": Claude Code and Codex are removed only when the
  record says Sandhill installed them, and their settings and sign-ins
  (`~/.claude`, `~/.codex`) always stay.
  Python (Huan, 2026-11-19; `docs/design/python-ready-plan.md`): the same
  exception covers conda environments Sandhill created, ipykernel it added
  (while still the recorded version) and Miniforge it installed (only
  while Miniforge holds nothing but Sandhill's environments and its base
  is as installed). Other conda environments and Pythons are never touched.
  Removing some (Huan, 2026-11-20; `docs/design/clean-footprint-plan.md`):
  What Sandhill Installed lists the record with ticks, and removes only
  the ticked items still as Sandhill left them, each with the tool that
  installed it (R's own installer, mamba/conda, pip, uv, Homebrew;
  CRAN's R through macOS's own password dialog). The record also keeps
  each install's tool, date and reason.
  Sandhill's own R and Python folder (Huan, 2026-11-20) is in the record
  too; removing it stops its kernels and deletes the whole folder, only
  one marked as Sandhill's.
- One folder (Huan, 2026-11-20; `docs/design/clean-footprint-plan.md`,
  section 6): Sandhill keeps its own files in `~/.sandhill` on a Mac, on
  Linux and on servers: `state` (history and the daemon's files), `bin`
  (the daemon on servers, ark, ruff, ty), `install`, `accounts`, `app`
  (the app's settings and servers list), `cache` and `environment`.
  The first start of a version with this layout moves the old folders
  there with one rename, only when nothing from before is running;
  until then, or if the move fails, Sandhill keeps using the old place
  and says so on Resources and Storage. XDG folders are no longer used.
  Sandhill writes no login item (no LaunchAgents plist, no systemd
  unit): the app starts the daemon when it connects. Outside the folder
  remain only what the system decides: one Keychain entry, macOS's
  folders for the app id, a temporary socket folder, and the Antigravity
  entry when asked for.

## Folder trust
- Folder trust (Huan, 2026-11-12): the first open of a folder asks once;
  trust is kept by the daemon on that device (never in the folder) and covers
  the folders inside. Untrusted is Restricted Mode: files, saved outputs and
  previews only; no kernels, Terminal, Jobs, Git, language servers or agents,
  enforced by the daemon. Projects opened before it existed are trusted. Tell
  agents per run where they allow it (Codex, pi); never edit an agent's
  configuration or answer its own trust question for the user.
  The question is a card in the quit and close cards' style (Huan,
  2026-11-16): the project's icon and "Trust x?" on one row, under it the
  device and the folder it is in, one sentence, then full-width buttons
  stacked: Trust Folder (default, Enter) and Open in Restricted Mode
  (Escape, or a click outside the card).
  An Explore a Table session (Huan, 2026-11-19) makes nothing beside the
  tables when they open: it starts in Sandhill's own storage (the daemon's
  `explore/` folder, trusted, holding links to the tables), which a
  session not kept leaves when its window closes. Sandhill asks "Keep the
  plots and steps from <table>?" on Save Session…, Export…, Open as
  Project, and closing after making a plot or export (Keep, Don't Keep,
  Cancel); the user may change the folder name ("<table> (Sandhill)" at
  first) and where it goes (the table's folder at first). Kept, it gets
  copies of the tables, `results/` and `steps.R`, and is trusted alone,
  not the folder around it.
