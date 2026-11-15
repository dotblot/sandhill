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

## Folder trust
- Folder trust (Huan, 2026-11-12): the first open of a folder asks once;
  trust is kept by the daemon on that device (never in the folder) and covers
  the folders inside. Untrusted is Restricted Mode: files, saved outputs and
  previews only; no kernels, Terminal, Jobs, Git, language servers or agents,
  enforced by the daemon. Projects opened before it existed are trusted. Tell
  agents per run where they allow it (Codex, pi); never edit an agent's
  configuration or answer its own trust question for the user.
