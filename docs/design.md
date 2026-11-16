# A Mac-native AI coding IDE for R/Python bioinformatics: recommended framework

*Research note, 2026-10-10, updated 2026-10-11: scope narrowed to Jupyter (`.ipynb`) and Quarto (`.qmd`/`.Rmd`); marimo dropped.*

*Original note, 2026-10-10. Nothing is built yet; this is a design recommendation. Claims about existing tools were checked on the web on this date (links inline). Where something is my inference rather than documented, it says so.*

---

## 1. The recommendation in one screen

| Layer | Pick | Why |
|---|---|---|
| Mac app | **GPUI** (Zed's Rust UI toolkit) through gpui-kit, with **gpui-webview** (the system's WebKit as a native view) for HTML outputs; gpui-kit's editor, and for terminals Ghostty's engine (libghostty-vt) drawn by gpui-cn's terminal element (Apache-2.0; switched from alacritty_terminal 2026-11-02, Huan). Replaced the first app, Tauri 2 with a SolidJS UI, in November 2026 (§3) | No Electron or bundled Chromium. Native drawing starts faster and uses about a third of the web app's memory; notebook outputs that are HTML (plotly, htmlwidgets, interactive tables) still get a web view each. |
| Mac ↔ server link | **System `ssh` (your `~/.ssh/config`, keys, ProxyJump)** carrying one multiplexed RPC stream to the daemon's stdio (msgpack or JSON-RPC) | This is the pattern VS Code Remote and Zed remote use. It needs no open ports and works through bastions. The app auto-uploads the daemon binary on first connect. |
| Server daemon | **One Rust binary (tokio)** running as a **`systemd --user` service with `loginctl enable-linger`** | It survives the Mac app closing, network drops, and your logout, with no tmux. It owns kernels, jobs, PTYs, the output store and the agents. |
| Kernels | **Jupyter protocol over ZeroMQ**: `ipykernel` for Python, **Ark** (Posit's Rust R kernel, MIT) for R | These are standard and reconnectable. Ark already speaks the variables, plots and help "comms" that Positron uses. Use the `runtimelib` / `jupyter-protocol` crates from nteract's runtimed. |
| Notebooks | **Jupyter `.ipynb`** (outputs stored in the file) and **Quarto `.qmd`** (and `.Rmd`) with a sidecar output store, plus one-click export of either to a single shareable `.ipynb` | See §4. Both run on the same kernels, and outputs are saved and come back on reopen. Your existing notebooks open as they are. |
| AI agents | Run the **official, unmodified `claude` and `codex` CLIs on the server**, under the daemon, each logged in with your own subscription **or an API key** (Anthropic or OpenAI format, with a custom base URL for compatible providers). Give both an **MCP server** exposed by the daemon ("run chunk", "read outputs", "list variables", "tail job log"). | This is the only clean way to use subscriptions without an API key. Running them server-side means agent work also survives the Mac closing. See §5. |
| Language servers | Run on the server through the daemon: **basedpyright** or **ty**, plus **ruff** (Python); **Ark's built-in LSP** and **Air** (R formatter) | Analysis runs where the code and packages are. |

**In short:** a native Rust app on the Mac (GPUI), a Rust daemon under systemd on Ubuntu, Jupyter kernels (ipykernel and Ark) behind both `.ipynb` and Quarto, sidecar output caches for Quarto, and Claude Code, Codex, Antigravity, Posit Assistant, OpenScience, Oh My Pi and pi running unmodified on the server, with an MCP bridge into your live kernels.

---

## 2. Architecture

```
┌──────────────── macOS (Apple silicon) ────────────────┐
│ GPUI app (Rust, one process; WebKit views for HTML)   │
│  ├─ Editor: gpui-kit (ipynb / qmd / Rmd / py / R)     │
│  ├─ Inline outputs: native, or a web view for HTML    │
│  ├─ Agent pane: a terminal attached to a server PTY,  │
│  │   or a chat view over `codex app-server`           │
│  └─ Local cache of outputs + docs (instant reopen)    │
└───────────────┬───────────────────────────────────────┘
                │  ssh host -- ~/.local/bin/sandhilld attach
                │  (one stdio stream, multiplexed RPC)
┌───────────────▼──────────── Ubuntu server ────────────┐
│ sandhilld  (Rust, systemd --user, linger enabled)          │
│  ├─ Session registry (SQLite)                         │
│  ├─ Kernel manager ── ipykernel / Ark (ZeroMQ)        │
│  ├─ Output journal: every IOPub message → SQLite +    │
│  │   content-addressed blobs (plots, tables)          │
│  ├─ Job runner: long scripts via `systemd-run --user  │
│  │   --scope` (survive even a sandhilld restart), logs     │
│  ├─ PTY host: shells and `claude` sessions you can    │
│  │   detach from and reattach to (your own "tmux")    │
│  ├─ Agent host: `codex app-server`, `claude` CLI      │
│  ├─ MCP server: notebook/kernel/job tools for agents  │
│  └─ LSP host: basedpyright/ty, ruff, Ark LSP          │
└───────────────────────────────────────────────────────┘
```

### Language choice (decided 2026-10-11)
Rust for both the daemon and the app, chosen for speed and low RAM. You are new to Rust, so AI agents write most of it and the Rust compiler acts as a strict reviewer. (The first app's UI was TypeScript; since November 2026 the app is Rust too.) Keep the Rust code in small, well-tested crates (protocol, kernel manager, output journal, ssh link, agent host) so each piece is easy for an agent to work on alone.

### Local mode on the Mac (added 2026-10-11)
The same daemon also runs on the Mac itself. "Local" is just another host:
- Build `sandhilld` for `aarch64-apple-darwin` as well as Linux. On the Mac it runs as a **launchd user agent** instead of a systemd service, and the app talks to it over a Unix socket instead of ssh. Everything above the link (kernels, output journal, agents, MCP tools, sidecar outputs) is shared code.
  - As built (step 2): the app ships `sandhilld` inside `Sandhill.app` and runs `sandhilld attach --start`, which starts the daemon in the background if it isn't running, so no launchd setup is needed. If the running daemon is another build (an older app left it running), `attach --start` stops it and starts its own; kernels survive that, as they survive any daemon restart. The remote case runs the same command through ssh (next section). The launchd agent in `packaging/` stays optional, for starting the daemon at login.
- Keep the OS-specific parts behind one small trait from day one: service install (systemd vs launchd), job isolation (`systemd-run` scopes vs process groups), and the transport (ssh stdio vs Unix socket).
- Closing the app window doesn't stop local work, because the daemon keeps running. But work does stop while the Mac **sleeps or shuts down**. The daemon should hold a sleep assertion while jobs run (what `caffeinate` does), and warn before you close the lid with work in progress.
- The kernels (ipykernel, Ark) and agents (`claude`, `codex`) run natively on Apple silicon.
- **Finding R on a Mac (2026-11-01).** The app starts the daemon with the `PATH` its login shell sets up, but that read can fail or take too long, and an app opened from the Finder has only the system folders. Without R on `PATH`, Ark (which runs `R RHOME` to find R) aborted with SIGABRT and IRkernel's `R` wasn't found. So kernels run, and their programs are looked for, with the daemon's `PATH` plus the folders a Mac login shell adds (`/etc/paths`, `/etc/paths.d`), Homebrew's and CRAN R's (`/Library/Frameworks/R.framework/Resources/bin`) that exist and it lacks. A kernel that still can't start says why: a missing program by name, else the last lines it printed (its log), and the daemon stops waiting as soon as the kernel exits.
- Local mode is also the easiest way to develop and test the daemon, so build it first.

### Servers over ssh (built 2026-10-11)
The `sandhill-link` crate (`crates/link`) is how the app reaches a daemon, on this Mac or on a server. For a server:
- **The system `ssh`.** The app runs `ssh -T` with `BatchMode=yes`, `StrictHostKeyChecking=accept-new`, a 15 s connect timeout and keepalives every 15 s. Because it is the user's own ssh, `~/.ssh/config`, keys, the agent and `ProxyJump` work as in a terminal. A server is anything you could type after `ssh` (a `Host` alias or `user@host`); the name is checked so it can never read as an ssh option.
  - `BatchMode` means ssh never prompts, because the app has no terminal to type into. So a server needs key login (a key, the agent or the macOS keychain). Password and two-factor prompts would need an `SSH_ASKPASS` helper; that is left for when someone needs it.
  - `accept-new` trusts a server's host key the first time, like answering "yes" in a terminal, and still refuses a known server whose key changed.
- **Check, install, attach.** Each connect first runs a short `sh` script on the server that reports its OS and CPU, the `PATH` the user's login shell sets up (`$SHELL -ilc env`, with a 10 s limit, so conda, `module` and `.bashrc` additions are there), and the SHA-256 of `~/.local/bin/sandhilld` (or of `sandhilld` on that `PATH`). If the server has none, or another build, the app pipes its own build through ssh into `~/.local/bin`, swaps it in with `mv` and checks the checksum. Then it runs `sandhilld attach --start` with that `PATH`, as in local mode: a daemon of another build is replaced, and kernels survive that. Every command is one `sh -c "…"` with no `$`, backquote or `!` inside, so whichever login shell sshd runs it with (bash, zsh, fish, tcsh) hands it to `sh` unchanged.
- **Builds.** `Sandhill.app` carries static Linux builds of sandhilld for x86_64 and ARM in `Contents/Resources/daemons/`, cross-compiled from the Mac with `cargo zigbuild` against musl. Static builds don't depend on the server's glibc, so they run on old CentOS and new Ubuntu alike. When the app has no build for a server's platform (a Mac server, for now), it uses the sandhilld already installed there and says so if there is none.
- **R with no setup (Ark).** The app also carries Linux builds of Ark, Posit's R kernel (MIT), downloaded from its GitHub releases when the app is built. The check reports whether R is on the login `PATH` and the SHA-256 of `~/.local/share/sandhill/bin/ark`; when the server has R and not this Ark, the app copies Ark there the same way, after sandhilld. The daemon offers that file as the `ark` kernel ("R (Ark)") whenever R is on `PATH`, without registering it with Jupyter, and a kernel the user registered as `ark` wins. Ark stays off `PATH` because KDE has a program called `ark` too. Ark links to glibc, so unlike sandhilld it needs a glibc Linux, which any Linux with R from the usual packages is. If copying Ark fails, the connection goes ahead and R needs a kernel installed on the server, such as IRkernel.
- **Nothing to set up on the server.** No root, no open ports, no systemd unit: `attach --start` starts the daemon as the user and it keeps running after ssh disconnects. The systemd unit in `packaging/` stays optional, for starting the daemon at boot.
- **Reconnecting.** When the connection drops, the app retries with backoff and the UI resubscribes from the last event it saw, so output printed while disconnected appears once it is back.
- **The start screen** is laid out like a macOS settings window (Huan, 2026-11-11; plan in [design/done/welcome-sidebar-plan.md](design/done/welcome-sidebar-plan.md)). A glass sidebar lists Devices (This Mac first, then saved servers, each with a status dot), a "+ Connect to Server…" row, Projects, Manage → Resources and Storage and Trusted Folders, and the crane with the version and "Elegance in discovery." at the bottom; it has no Recent section. Since 2026-11-12 ([design/done/idle-agents-welcome-window-plan.md](design/done/idle-agents-welcome-window-plan.md)) Welcome is its own window, half the width and height of the display (at least 800×600), beside one maximized main window, opening on This Mac's page; Projects lists pinned, open and running projects of every connected device ([design/done/welcome-projects-resources-plan.md](design/done/welcome-projects-resources-plan.md)); devices can be renamed and projects given built-in icons, as this Mac's settings. Resources and Storage is a page, not a dialog. A device's page shows: a left-aligned title with Connect/Disconnect for servers, a state subtitle, a per-server "Connect automatically" switch (default on), the projects as one grouped inset list, a right-aligned "Open Folder…" and the note about kernels surviving. Connection state is shown by dots and buttons, never by a switch; Disconnect drops only Welcome's link and never stops server work. Earlier the same day it was one panel with device tabs; before that, a card per device. Connect to Server opens a centred dialog. Choosing a folder (Huan, 2026-11-14; [design/done/folder-chooser-plan.md](design/done/folder-chooser-plan.md)): on this Mac, Open Folder and the plot window's folder choice use the system's folder window, since the app and the daemon see the same files, and the chosen path still goes to the daemon to open; folders dragged from Finder onto Welcome open the same way. For servers, the folder dialog (`folder_dialog.rs`, reading folders through the daemon, which marks Git and Sandhill folders when `ListDir` asks) has places, a clickable path, filtering, keyboard use and New Folder; a start folder that does not exist yet opens its nearest parent. The server list is kept in the app's `servers.json`. A window showing a project on a server is titled "project — server".

Not yet: password prompts, forwarding ports for the Viewer pane, Mac servers (the app would need to ship its own Mac build as one), and a server that can't reach `~/.local/bin` (a read-only home).

### Device resource, storage and removal management (approved 2026-11-08)

The status-bar device menu exposes a management window for this Mac and saved
SSH destinations. The current device alone is selected initially; selecting
other devices is explicit. Each device reports its resources, affected project
paths, storage categories and individual operation results. An offline device
is not a successful cleanup. Inspection of an explicitly selected device may
connect to it; ordinary Disconnect/Reconnect retains its existing meaning.

Stopping all resources first requests shutdown of Sandhill-owned kernels,
terminals, jobs and agents and confirms their exit. Remaining processes require
a separate force confirmation. Never identify ownership by a generic executable
name such as `python` or `R`; unverifiable processes remain reported rather than
being killed. Stop the daemon only after resource shutdown succeeds. Processes
remain independent during ordinary app exit, link loss and daemon upgrades.
Terminal hosts started by builds before managed stops (their `host.json`
holds only `host` and `pid`) cannot stop without escalating: their only
request, Hangup, sends SIGHUP and then SIGKILL after 3 s. Stop All reports
them; explicit Force Stop sends that Hangup, the same request closing their
tab used, and waits for the recorded exit (Huan, 2026-11-11).
An incomplete Stop All leaves the daemon refusing ordinary work, but an
explicit reopen of that device replaces the daemon; the replacement adopts the
remaining resources and lists them for another Stop All. Builds before
2026-11-11 refused that reopen ("resource shutdown is incomplete"), leaving the
device unreachable; the attach bridge now answers that refusal by stopping
only the daemon (StopDaemon keeps resources running) and starting a new one.

Quit defaults to keeping background work. The alternate choice is **Stop all
background resources and quit**, not daemon-only shutdown. Save all edited
documents first; failure or unconfirmed remaining resources keeps the app open.
Devices shared by several project tabs are operated on once.

Storage inspection separates programs, output/history, logs, regenerable cache,
app settings and project-local data. Cache has a bounded automatic cleanup
policy and active previews are protected; saved outputs and conversations are
not disposable cache. Destructive cleanup previews paths and sizes and requires
confirmation; historical data is never selected by default. Project `.sandhill`
directories are report-only in device cleanup, not recursively erased.

Removal uses installation provenance (exact path and checksum), not a filename
guess. Changed, shared, externally installed or unverifiable components remain
with an explanation. Do not remove R/Python, conda environments or standalone
agent CLIs. Stop resources, remove only owned service registrations/components
and Sandhill tool registrations, optionally remove selected backend history,
then stop the daemon. Preserve data by default. Report each failed step and
remaining path, including unreachable devices. Optional removal of the current
Mac app moves the bundle to Trash last and only after all selected device
operations succeeded; development checkouts are not app bundles to delete.

SSH upgrades also support binaries installed before provenance records existed.
Before replacing an unrecorded daemon or Ark, preserve and verify its exact
bytes beside the original at `<target>.legacy-sha256-<checksum>` and report the
backup path. A conflicting backup or mismatched existing ownership record still
blocks replacement. Only the newly installed binary is owned by Sandhill;
legacy backups remain protected and are not silently adopted for uninstall.

### Compact status and monitoring (approved 2026-11-08)

The status bar keeps the device name and a colored state dot, with full state
text in the tooltip/menu. CPU, memory, disk and available GPU metrics use small
static rings beside that device; details report temperatures only when the
host exposes them without elevated privileges. Unknown readings are not zero.
GPU collector failures remain visible even when another GPU can be measured.

The opposite end shows logged-in AI account quotas reported by the unmodified
CLIs. Use their actual window durations and reset times; show the 5-hour and
7-day windows only when supplied. Missing, stale or expired readings stay
unavailable rather than implying free quota. Never extract OAuth tokens to
query provider endpoints or send a conversation prompt just to fetch limits.

Antigravity 1.5.5 exposes `/usage` through the official read-only
`agy --print /usage --output-format json` command. Keep its Gemini and
Claude/GPT quota groups separate and convert remaining fractions to usage.
The CLI result does not identify the account: display it as an anonymous full
CLI snapshot, never merge it with an account feed. The same built-in handles
`/usage` from Sandhill's chat; it must not be sent as a model prompt through
stream-json or change the conversation. Command-line mode runs agy's native
TUI, whose own `/usage` command handles terminal input.

Quota rings show their window inside (`5h`, `7d`, or a compact duration).
A provider's popup shows its larger, bold name and logo, then the windows
with resets, then its display switch and refresh, then "All AI accounts…"
(Huan, 2026-11-11). Claude Code 2.1 has no headless, read-only usage command
(`/usage` is an in-session screen), so Claude quota appears only from the
status line of a running Sandhill Claude conversation and the popup says so.

Console controls share the Console/Terminal/Jobs tab strip, only on the Console
tab while expanded. Outer left/right pane margins are 7px; other gaps remain.
Terminal replay reconstructs the screen without sending historical query
responses back to the process; live terminal queries must still work.

### Workspace controls and session retention (approved 2026-11-10)

Preserve the existing glass, fonts, pane layout, icon source colors and accent
palette. These changes affect controls and session lifecycle, not the visual
direction. No new external services, credentials, dependencies or installations.

1. **Stop then reopen**: distinguish a reachable daemon from one accepting work.
   `StopResources` currently latches maintenance; a same-build attach accepts
   that daemon based on ping/build alone. Trace why quit left that process
   available, confirm actual daemon exit rather than only its stop reply, and
   make an explicitly reopened device recover a stopped daemon safely. Do not
   clear maintenance or restart during an active cleanup/uninstall, and do not
   restart kernels stopped by the user. Failures must remain visible.
   Legacy inventories can include unverifiable independent work. They do not
   block a daemon-only cooperative restart: wait for the legacy lifecycle gate
   and confirmed daemon exit, retaining every independent process.
2. **Project close**: put an X immediately after every project name, active or
   inactive. Reuse the existing project-close/save confirmation path. Closing a
   project must not stop its background work; closing the last shows Welcome.
3. **Resource icons**: replace CPU/memory/GPU/disk words in the left status bar
   with existing control icons; retain usage rings and click details, including
   unavailable readings and multi-GPU errors.
4. **Quota controls**: match resource-button geometry and neutral hover/selected
   styling. No quota-details tooltip on hover; click toggles details and
   outside click/Escape dismisses. Each agent row has a persisted display
   switch and refresh-now action. Disabled agents disappear from the status bar
   and stop scheduled quota queries, but remain configurable from an account
   control even when all are disabled. Manual refresh is explicit and bypasses
   cached quota values; disabling must reach daemon provider selection, not
   merely hide rendering. Do not change agent login or running conversations.
5. **Welcome theme**: reuse the workspace theme toggle at the top right.
   Correct secondary/path/status/footer text against both the glass wall and
   cards in dark mode without changing glass opacity or recoloring all surfaces.
6. **Console sessions/history**: move the unbound kernel picker immediately left
   of the minimize control. Switching between unbound R/Python kernels retains
   sessions instead of calling Shutdown, reuses an existing selected session,
   and restores its journaled input/output. Add a per-session command-history
   popup showing the newest 50 entries; retain at most 100 command-history
   entries, removing oldest beyond that. Selecting history fills the composer
   without executing. This bounded command list must not delete execution/output
   journal evidence used by notebooks or analysis checks. Handle switching while
   a session starts and while another session is busy.
7. **Collapsed file groups**: group same-type document icons in inactive
   capsules, ordered by first occurrence. A singleton has no count; multiple
   files show one original-color icon and their count. Expanded file labels,
   close controls, dirty state and non-file views remain available.

Implementation spans GPUI, daemon and protocol. Verification uses targeted
lifecycle/session/history/quota checks and an isolated native app run at normal
and minimum window sizes in both themes. Existing user sessions, project data
and installed applications are not stopped, migrated or replaced for verification.

### RStudio and Positron borrowing (approved 2026-11-11)

Huan accepted the first four priorities from the RStudio/Positron comparison:
editing basics (completion and diagnostics, find/replace, folding/outline,
RStudio R shortcuts), data viewing (object explorer, table column visibility
and import code, fixed-size plots), convenience (terminal file links, Quarto
render jobs, kernel recommendation reasons, pinned recents, focus mode) and AI
context (Data Explorer state, Viewer reading, notebook Explain/Improve).
Debugger, package development, profiling, spelling, snippets, databases,
publishing, extensions, `.RData` and multi-root workspaces are not included.
Huan accepted the plan's defaults: outline as an editor-action-row popup, ty
and ruff for Python (Sandhill-owned pinned copies only when absent), and new
conversations for notebook Explain/Improve. The implementation plan is in
[design/done/rstudio-positron-borrowing-plan.md](design/done/rstudio-positron-borrowing-plan.md).

Implemented 2026-11-11. Decisions made while building:
- Python language hosts are shared per project; R hosts are keyed by the Ark
  session, because Ark's language server reads that session's live globals.
  Pinned Ark returns empty Jupyter `complete_request` replies, so R completion
  comes only from Ark's LSP; the R Console therefore has no completion.
- Language features attach when an editor is clicked/focused by the user or
  edited; diagnostics are not computed for files that were only opened.
- Render (icon), notebook AI (icon) and Clear outputs (icon) are icon-only so
  Save stays visible in the 29px row at the default window width. At the
  minimum width the row still clips, as before; Render is also in the palette.
- The plot size control is a short `Fit`/`W×H` label for the same reason.
- Bitmap plots now use the shared lenient base64 decoder: Ark sends PNG data
  without padding, which the strict decoder rejected.
- Known limit: the editor component anchors completion below the caret, so
  Console completion near the window bottom can extend past the window.

### Huan's 2026-11-11 feedback (approved)

The plan is in
[design/done/ui-feedback-2026-11-11-plan.md](design/done/ui-feedback-2026-11-11-plan.md).
Decisions made while building:
- Console plots stay inline. Sending a plot (or all of a session's plots) to
  Plots is explicit; auto-send is an opt-in switch per device. Sent plots are
  remembered locally, bounded, so reconnects restore only what was sent.
- The Console has its own session target, separate from the active editor's.
  Activating or running a document moves it to that document's session; a
  document's own kernel is never changed by Console choices.
- Opening documents shows the tab at once with a thin progress bar. Every
  cell's editor is still created, so whole-document actions (Run All,
  find/replace, outline, saving, agents) see the whole model. Only cell,
  output and add-row views are built near the viewport, with reserved heights
  and scroll anchoring. Initial script folding runs in the background. On a
  1,500-cell notebook the first draw fell from about 102 ms to 8 ms, and a
  50,000-line script's editor setup from 278 ms to under 1 ms.
- The window busy cursor was removed; the pointer never changes while loading.

### Huan's 2026-11-11 feedback, third batch (approved)

The plan is in
[design/done/ui-feedback-2026-11-11-batch3-plan.md](design/done/ui-feedback-2026-11-11-batch3-plan.md).
Decisions made while building:
- A new `.qmd` takes its first chunk's language from the kernel the document
  will use (remembered choice, project default, first installed kernel), or
  starts with only the YAML header when no kernel is known; `.Rmd` stays R.
  Before, every `.qmd` began with an `{r}` chunk, so Python code typed into it
  was analysed as R and its names were unknown to later Python chunks.
  Existing `{r}` chunks stay R.
- Pane headers reset the surface alpha to 0.82 rather than multiplying the
  glass alpha, so a header is never less opaque than its pane. Scrolling
  content is padded below the header, scrollbars start beneath it, and
  programmatic scrolls (jump to cell, find, outline, file reveal) keep their
  target below it. Headers are the last child of their pane, so popups from
  other panes still draw over them.
- The plot window keeps the original picture's pixels until it is resized,
  so it opens looking exactly as before; Size follows window then sets the
  export size from the room at 96 points per inch.
- Switching a conversation between chat and command-line mode needs a bridge
  that reports its view mode. A bridge without it, on a daemon whose binary
  matches this app's bundled server daemon, is restarted (idle only) and then
  switched. On an older or unverifiable daemon the AI pane says that device's
  Sandhill service must be updated first (it is, when the device is connected
  from an app build carrying server daemons); a dev run has none. Every AI-pane
  error banner can be closed and clears on the next success.

### Huan's 2026-11-11 feedback, fourth batch

Small changes Huan described directly (no separate plan):
- AI composer: Up/Down move through the `@`/`#` list, Enter/Tab pick, Escape
  closes. Kit's input binds those keys to actions (`MoveUp`, `MoveDown`,
  `IndentInline`, `Enter`), so the composer captures the actions, not key
  events. The history's Delete? confirmation is white on solid red.
- Data Explorer and notebook tables: see *Filters, search and sorting* and
  *Read-only table selection* below. Huan chose copy only (no cut/paste) and
  values without a header row.

### Why a custom daemon and not Jupyter Server?
Jupyter Server keeps kernels alive after the browser closes. But in the classic setup, outputs a kernel emits while no client is attached go nowhere: IOPub is publish/subscribe, so a disconnected client misses them. (That is my inference from the protocol; JupyterLab's newer server-side document work narrows this gap.) Your core need is "close the laptop, come back, see everything that happened". So the daemon itself must subscribe to IOPub and journal every message to disk. On reconnect the Mac asks for "everything since sequence N" and replays it. This is the same idea nteract 2.0's `runtimed` uses locally: a per-user daemon that owns kernels, with outputs kept as content-addressed blobs ([architecture](https://mintlify.wiki/nteract/desktop/development/architecture-deep-dive)). You are adding "remote" and "journal while nobody is watching".

### Surviving restarts
- **Mac app closes or the network drops:** nothing on the server notices. The daemon keeps journaling.
- **You log out of the server:** `loginctl enable-linger $USER` keeps user services running.
- **The daemon itself restarts (an upgrade):** start kernels and jobs in their own systemd scopes (`systemd-run --user --scope`) so they are not its children. Jupyter kernels are network servers described by a connection file, so a restarted daemon can reconnect to a running kernel. Outputs emitted during the few seconds the daemon is down are lost. Batch jobs write to log files, so they lose nothing.
- **Long analyses** (alignment, Seurat/Scanpy pipelines): encourage "run as job" (script → log + result files) rather than a 10-hour notebook cell. The UI can still render the job's plots and tables inline.
- **Jobs, as built (October 2026):** a job is a terminal of kind `job`, so it gets what terminals already have: its own process group under a host that outlives the daemon, a log kept on disk and replayed to any client, and its exit code. It runs in the user's login shell, started interactive so conda and modules are set up as in their Terminal. An R or Python script runs with `Rscript` or `python` from the environment of the project's kernel for that language, with that environment's `bin` put first on `PATH` after the shell's own setup (which may activate conda's base), so a job sees the packages the project's sessions do. Shell scripts, Nextflow (`nextflow run`) and Snakefiles (`snakemake`) run too, and so does any command line. CPU and memory come with `system_stats`, summed over the job's processes, CPU as `top` counts it. The app tells of each job that ends, including those that ended while it was closed, by remembering per project the latest end it told of. On a Mac, the Mac stays awake while a job runs (2026-10-29): the daemon starts `caffeinate -i -s -w <pid>` beside each job, which stops the Mac from sleeping when idle (and on power, at all) until the job's program ends, whether or not the daemon keeps running; a laptop still sleeps when its lid closes on battery. Agents see jobs too (2026-10-29): the MCP tools `list_jobs` (what each runs, where, and whether it runs, finished or failed, with its exit code and for how long) and `read_job_log` (the end of a job's log as plain text, colours gone and a progress bar as it last showed), so an agent can follow a pipeline or explain why it failed. Not yet: `systemd-run` scopes (a job is killed with the host if the user's whole session is), and keeping a Linux machine awake.

---

## 3. Mac app choice: GPUI (first Tauri)

> **Update 2026-11-02: the GPUI app is Sandhill.** Once it did everything the
> Tauri app did (steps 1 to 7 of "Sandhill on GPUI"), Huan tried a Mac build
> and chose to retire the Tauri app: `app/` was deleted, and the GPUI app
> became Sandhill.app with the Tauri app's bundle id, `dev.sandhill.ide`, so
> it replaces it and keeps its data folder (the list of servers). The Tauri
> app is in the git history (last in commit d3ff9f6).
>
> **Update 2026-11-01: moving to GPUI.** A trial GPUI app (`gpui/`, built
> with gpui-kit and gpui-webview for HTML outputs) started in 0.4 s against
> the Tauri app's 0.8 s and used about a third of the memory with a 400-cell
> notebook open. After trying a Mac build, Huan chose to move Sandhill to it.
> The HTML problem below is handled by gpui-webview (the system's WebKit as a
> native view, clipped with gpui-kit's GPUI Fast engine). The port follows
> the plan "Sandhill on GPUI": look and layout, agents, the right-side panes,
> Git, the remaining hard parts, then the switch, when `app/` is deleted. The
> table below is the original choice, kept for its reasons.

| Option | Verdict |
|---|---|
| **Tauri 2** (recommended) | Rust backend, system WKWebView, small binaries. One codebase for the editor, notebook cells and outputs. Downside: WKWebView is Safari's engine, so test plotly and htmlwidgets there. That is fine in practice. |
| Native SwiftUI/AppKit + WKWebView only for outputs | The most "Mac", with the lowest idle RAM. But you would build a code editor from scratch (or STTextView/Runestone) with LSP, multi-cursor and folding. That costs months before any data-science feature exists. A good choice only if UI polish matters more than time. |
| Fork **Zed** (Rust/GPUI) | The fastest editor available, with ACP agents, SSH remote and a REPL already built. But GPUI has no HTML engine, so htmlwidgets, plotly and interactive tables are hard. Its upstream also moves very fast. Worth studying, not forking. |
| Electron / VS Code fork | Excluded by you. It is also what Positron, Cursor and Erdos are. |

### Layout (decided 2026-11-02, Huan)

Three panels across the window, 1:2:1, with 8 px between them and the
window's left and right edges (10 px until 2026-11-03). The left panel holds, top to bottom:
Variables, Kernels, Help and Packages (one tab group); Files, Git, Plots and
the Viewer (another), each half of the panel's height at first (2026-11-03). The middle has the editor with the Console, Terminal
and Jobs under it. The AI pane is the right panel. The machine's resources
(CPU, memory, GPUs, disk) follow the connection item at the left of the status
bar (Huan, 2026-11-08), and the title bar has a light/dark theme button.
(Revised the same day: the first version
was 1/5, 2/5, 2/5 with Kernels and Resources as sections of the left panel.)
The descriptions of panes below predate this and place them where they used
to be.

**Console and new conversation simplification (Huan, 2026-11-08).** With no
document open, selecting a Console kernel starts it immediately; no Start
button remains. Re-selecting a ready or starting kernel keeps that session;
selecting it after shutdown/failure starts it again. Rapid kernel changes
or cancellation cannot let an earlier start overwrite the newest selection
or leave its unused kernel alive. Notebook kernel selection keeps its
existing run-on-demand behavior.

The New conversation page has no agent-button row. The composer's
provider/model menu remains the selection point for all existing agents.
Posit Assistant stays selectable there, and its new-conversation composer
keeps the provider picker so the user can switch back. PA's own model/mode
controls are not duplicated. Resource details are anchored above the selected
left-side status indicator and constrained to the window.

Each of the four pane groups (left top, left bottom, editor, Console group)
has Collapse and Expand vertically buttons (Huan, 2026-11-05). Collapse
leaves its 29 px title bar and gives the rest of that column to its sibling;
vertical expansion does the opposite. Repeating the corresponding action
restores the normal height shares, which are not overwritten by either
operation. Column widths stay unchanged. The Console group's close button
is removed; collapsed contents stay alive, including their kernels,
documents and terminal sessions. Native notebook/Viewer pages are hidden
while their containing pane is collapsed. Narrow tab strips scroll
horizontally so the height buttons remain reachable. The AI pane already
fills its column and has no height controls.

**Brand accents and compact editor (Huan, 2026-11-08).** Kit's default neutral
surfaces and glass remain, with interactive colours derived from the crane's
navy `#142844`; dark mode uses a lighter blue with readable contrasting text.
Primary buttons, focus rings, links, slider fill and selected rows/tabs share
these tokens in Kit and custom UI. Code syntax, source icon colours, semantic
red/amber/green, charts and terminal ANSI hues are unchanged.
Editor pane controls and actions occupy a single 29px row, separate from the
window's project capsules. Actions scroll horizontally when space is tight;
height controls stay pinned. The kernel picker contains its state dot rather
than a separate status phrase. Console puts that dot after the filename, or
in the picker with no file. Status and failure details remain available on hover.
The picker lists only runnable R/Python kernels, including those discovered
inside conda/mamba environments. It does not show a separate environment
inventory, disabled environment rows or an inventory-query warning (Huan,
2026-11-08). Existing kernelspec discovery and command deduplication remain.
Per-document kernel choice is saved immediately in the daemon's existing
project state at `documents[absolute_file].kernel`. Recovery prefers a live
session, then remembered document choice, then project default, then notebook
metadata/language fallback only when no choice exists. Missing remembered or
project kernels remain visible and require explicit selection before running.

**Read-only media tabs (Huan, 2026-11-08).** PDF and SVG/PNG/JPEG/GIF/WebP/BMP/
TIFF/ICO files open in middle document tabs without a kernel. Existing daemon
`serve_page` and port tunnels stream the binary to a private `0600` temporary
file, removed with the preview. PDFKit supplies page navigation, fit and zoom;
AppKit supplies image decoding, animation and scrolling. SVG uses existing
resvg with no scripts or external image references, and a shared font cache.
Fit is applied after native view layout; explicit zoom survives resizing.
Native surfaces hide on tab/project changes and editor collapse, and share
GPUI Fast's clipping/overlay composition. Preview limits are 512 MiB per file,
32 MiB for SVG source and 32M SVG raster pixels; corrupt/unsupported files
show an error rather than opening externally or starting a kernel.

In notebooks, a cell shows its number (#3) in the gutter. Between the cell
and its output a line shows the run's number, as Jupyter does (`[3]`), and
how long it took (2026-11-03, Huan: this replaces "no run count" and the
time under the number). Cells take the notebook's whole width. Output text can be selected and copied, and a plot
under a cell can be saved (the same dialog as the Plots pane's) or copied.
Each code cell shows line numbers starting at 1; Markdown does not.
R and Python function calls italicize only named argument identifiers in
`name = value`, not the equals sign, value expressions, formal parameters,
properties or ordinary variables.
Python `from`, `import` and import-alias `as` tokens are also italic;
unrelated `as`/`from` keywords and imported names retain their existing
styles. R `library()` and `require()` call names, including qualified
`base::library()`, are italic in the existing function colour. Ordinary
references and definitions of those names are unchanged (2026-11-06).
The output box is inset 8 px from each side of the code box and has a
1 px border on its sides and bottom, with rounded bottom corners and no
top border or top curves. The sides extend to the code box's lower edge,
enclosing the run metadata as well as the outputs. Its background and
status colours stay unchanged (Huan, 2026-11-05).
Clicking code or output selects the owning cell without taking editor focus
or consuming text-selection gestures. Selected code/output frames change
border colour, not width; no left selection stripe remains. Text outputs use
I-beam cursors and retain copy/selection (Huan, 2026-11-08).
`output_text.rs` retains a small rendering owner per plain output/table text
participant. Its selection-change subscription invalidates that owner so
drag highlights can repaint through Fast's retained subtree path, without a
timer or a whole-notebook redraw. Handles/subscriptions leave with the output.
Verification must inspect a changed highlight before mouse-up, not merely
successful copy after release.

Document reads and parsing are tracked until completion, including failed
and concurrent opens. The Mac requests an arrow with a clock badge while
the shown workspace is loading, through AppKit tracking areas on the
existing native view (`busy_cursor.rs`), without intercepting input.
Parsing is off the UI thread; constructing GPUI editors stays on it.
Console and AI input frames are transparent, showing the same pane
background as their output above; their rounded borders remain visible
and use the accent when focused (Huan, 2026-11-05). Inactive frames use a
slightly stronger neutral border for contrast (Huan, 2026-11-08).

Notebook cells work as Positron's notebooks do (decided 2026-11-03, Huan's
batches 1 and 2; built from Positron's documentation and behaviour, not its
code, which is Elastic License 2.0). Pointing at a cell shows its tools in a
bar just above its top right corner, clear of the code (2026-11-03): Run
first, then run the cells above, run it and those below, fold or clear its
outputs, move it up or down, and delete it. Run uses the same round hover
background as the other icons (2026-11-05). The line under the
cell says how its run went: waiting while it waits its turn, the time so
far while it runs (the oldest unfinished run of the kernel is the one
running), then how long it took, red if it failed. Rows between cells add a code or
Markdown cell. Long outputs scroll in a box 480 px tall; while the pointer is over such a box, the wheel scrolls only the box, not the notebook. The notebook bar picks the kernel from one list: R kernels, then Python environments (conda and mamba ones included). A data frame
(`<table class="dataframe">` from pandas, polars or IRkernel) shows as a
grid, with a button that opens it in the Data Explorer when the cell's last
line names it (`df`, `df.head()`, `head(df)`). Esc leaves a cell's editor
and picks the cell; then A/B add a cell above/below, D D deletes (Z brings
it back), M/Y make it Markdown/code, ↑/↓ or K/J move between cells, Enter
edits, Shift+Enter runs and moves on. Cells are dragged to a new place by
their gutter. Moving and changing cells is for Jupyter notebooks only:
a Quarto document's cells are found in its text and its unlabelled chunks
are known by their position. Adding and deleting work in `.qmd`/`.Rmd` too
(Huan, 2026-11-11): add rows and A/B insert a fenced chunk or prose after
any YAML header; D D deletes a cell and Z restores its exact source bytes.
Other source, YAML and chunk options remain byte-for-byte unchanged.
The editor's action row switches each Quarto document between Notebook
and whole-file Source views, retaining unsaved text and cell outputs for
the session. Saved `.ipynb` files no
longer keep run counts (`execution_count` is null), so running a notebook
doesn't change the file. Clearing outputs also clears them in the daemon
(`clear_cell_outputs`), so they stay cleared when the notebook is opened
again.

### The look (added 2026-10-11)

The app follows the UI mockup, the agreed final layout, kept in
`docs/design/ui/`: `spec.md` describes it, `sandhill-layout.html` is the
clickable mockup (light and dark) and the PNGs are screenshots of it. Where
the spec and the mockup differ, the mockup wins. The app follows draft 17
(2026-10-12) and, since 2026-10-25, the title bar and machine tabs of drafts
20 to 35; the top of `spec.md` lists what each draft changed.

**Unified Kit colours (Huan, 2026-11-08; approved).** GPUI Kit's bundled
Default Light/Default Dark themes are the single source of interface colours,
for both Kit widgets and Sandhill's own title bar, project capsules, panes,
menus, dialogs, inputs, status bar and data views. Terminal default text,
background, cursor and the sixteen ANSI colours use the same theme, including
its base colour families. Kit's default primary colour replaces the macOS
system accent; no system-accent override remains.

This is a colour-only change: preserve the native window blur and existing
surface opacity, panel layout, sizes, fonts, radii and interactions. Transparent
Console/AI inputs and window underlays stay transparent so the blur remains
visible. Code syntax keeps the current Xcode colours and keyword/argument
italic rules; editor background, foreground, line numbers and selection follow
Kit. Preserve original file/brand icon colours, user-provided data colours,
images, plots and embedded HTML. The app maps resolved upstream theme values
instead of maintaining a second hardcoded interface palette.

Verify light/dark switching on existing views, contrasting text on filled
semantic controls, distinct CSV/TSV column colours, terminal ANSI output,
retained transparency and unchanged code syntax. Build a local test app;
report native screenshot limits if the verification Mac is locked.

**Project capsules (Huan, 2026-11-06; replaces machine tabs).** The 31 px
title bar has flat panel/search/menu buttons after the traffic lights and a
flat theme button at the right, with hover backgrounds but no capsule
around either group. The centre has a separate capsule for each project.
Project identity includes its machine and canonical project path, so two
projects on the same Mac or server remain open independently.

Only the active project expands: its folder name, a vertical divider, then
its document tabs with file icons, names, selected state, unsaved markers
and close controls. Inactive capsules show the project name and file icons
only, with full file paths on hover. Clicking a project name restores its
last document; clicking an inactive file icon selects both that project
and document. Existing Data Explorer and Git diff tabs move with the file
tabs; labels are not duplicated in the editor pane, whose height controls
and Notebook toolbar remain. The capsule row scrolls horizontally when
needed, leaving the side controls clear. Same-named projects have machine
and path tooltips. Right-clicking a capsule asks before closing that
project, without stopping its kernels or agents.

The left status bar again shows the active project's device and connection
state, and clicking it opens the existing connection details, reconnect,
disconnect and daemon-log actions. No run-shortcut hints return. Resource
indicators follow the connection, then problems and notices. Switching a
project switches the whole workbench, including Console, plots and AI,
while native pages belonging to the previous project are parked.

Window project lists and the active `(host, path)` are saved in
`gpui-windows.json`. Old records whose selection was only a host are read
as that host's existing project and rewritten in the new format on the
next save. The Start screen opens projects as new capsules or activates an
already-open one instead of replacing a same-device project. No notebook
format or daemon protocol change is involved.

**Pane tabs (built 2026-10-25).** Every pane's tabs sit in a capsule like
the title bar's, the picked tab a raised pill, and the tab bars (and the
editor's kernel bar) are liquid glass floating over the top of their pane:
the content starts under them and scrolls up beneath them, blurred. Only the
bars blur; the panes still don't. The editor keeps the cursor clear of the
bars. Only the title bar drags
the window. No text is smaller than 10px, the smallest size macOS uses.

**The file tree (built 2026-10-25)** follows drafts 26–28: a chevron column,
an icon column, 16px per level and plain guide lines. New files and folders
are named in a field in the tree, from the + menu (in the selected folder),
a folder's hover buttons, the right-click menu, ⌘N and ⌥⌘N; a name such as
`raw/samples.csv` makes the folder too, and new Quarto documents, R scripts
and notebooks start from a template (`app/src/panes/files.rs`). The right-click
menu renames, duplicates, copies the path and moves to the Trash, plus
Reveal in Finder on this Mac, and Download to this Mac and Delete… on a
server. These go through the daemon (`make_dir`, `move_path`, `copy_path`,
`delete_path`, `read_bytes`; see `docs/protocol.md`), so they work the same on
a server. The daemon never replaces anything: a clash comes back as an
`exists:` error, which the field shows in red. Renaming or deleting closes
the documents open from it first, and refuses while one has unsaved changes.
The tree sorts by name, size (folders by the sum of their files) or date
modified, folders first; the order is remembered on the Mac.
Every file row shows its formatted size after the name, including supported
editor formats and empty files (`0 B`). Names truncate before the size; folder
totals are still calculated only when sorting by size.

**Import and column stats (built 2026-10-25).** Variables has an Import menu,
as in RStudio: for R, text (base and readr), Excel, SPSS, SAS, Stata, R data
and 10x Genomics; for Python, pandas, Parquet, AnnData and 10x. Each opens a
dialog that browses the daemon's machine for files it reads, names the
variable after the file and shows the code before running it in the session,
so the run shows in the Console (`app/src/editor/imports.rs`). The Data
Explorer's column list and stats hide with **Hide column stats**, remembered
on the Mac.

**Approved file/UI extension (Huan, 2026-11-07).**
Keep the existing macOS palette, panes, project state and daemon-only file
access. This extends the shared file-open dispatch rather than adding a
second local-only file reader.

- Thinking effort keeps Kit's standard slider, with labelled discrete tick
  marks for the current model/provider's available levels. No explanatory
  paragraph. Dense labels must not overlap or widen the fixed model menu.
  *Superseded 2026-11-12 (Huan):* one segmented bar after a Dribbble
  reference, the chosen level's name in its fill (`app/src/agents/effort_bar.rs`).
- Hide the title-bar project `+` on the welcome screen only; existing project
  windows retain it. Prefix each expanded/collapsed project capsule's name
  with the existing local-device or remote-server icon. *2026-11-12:* Welcome
  draws no capsules; its Projects page lists pinned, open and running
  projects (`docs/design/done/welcome-projects-resources-plan.md`).
- Every dropdown trigger toggles its own popup. Fix outside-mousedown and
  trigger-click ordering so a second click closes rather than reopens it;
  clicking another trigger switches menus, outside clicks and Escape close.
  Apply this to custom popups as well as built-in selectors.
- Files' New menu adds Python script, initially `untitled.py`, using the
  existing inline naming, conflict checks and editor-opening flow.
- Clicking CSV/TSV first offers text editing, import into an existing live
  R/Python session, or cancel. Import reuses the existing code-preview/run
  path and allows selecting a live session; it never silently starts one.
  Without a live session, import is disabled with an explanation.
- Treat 5 MB as 5,000,000 bytes. Files above this limit cannot open as text;
  only import or cancel is offered. Check the size on the daemon and bound
  the text read too, so a stale tree size or file growth cannot bypass it.
  At exactly the limit text opening remains allowed.
  Use the original `read_bytes` API for metadata and bounded chunks, including
  EOF/version checks, so this also works with older remote daemons. Do not
  depend on an optional `stat_files.size` or a newer bounded-read RPC.
  Import choices show readable environment, session number when needed, and
  project-relative working folder, never the internal session ID.
- Text mode remains an editable delimited file, not an imported data frame.
  Give adjacent columns distinct, theme-readable text colours using a
  repeating palette; delimiters, quotes and original text are preserved.
  Treat the first logical record as the header, with a neutral grey
  background fixed during vertical scrolling and horizontal scroll aligned
  with the body. Respect quoted delimiters, escaped quotes and multiline
  quoted fields. Saving uses existing external-change checks.
- Markdown opens as source with a rendered-view toggle in the same document
  tab, without requiring Quarto or an R/Python kernel. Render the current
  edited content using the existing Markdown renderer; do not execute scripts
  embedded in Markdown.
  Supported Markdown extensions are `.md`, `.markdown`, `.mdown` and `.mkd`,
  case-insensitive. `.mdx` and extensionless names are not inferred as Markdown;
  Quarto and R Markdown retain their existing document flow.
- Shell scripts open in the text editor with Bash/Shell highlighting,
  including `.sh`, `.bash`, `.zsh` and extensionless files with a recognised
  shell shebang. Opening never executes them or attaches an R/Python kernel.
- Other unsupported files offer opening with the system default app or
  cancelling. Local files open on this Mac; for remote files the prompt
  explicitly offers download to this Mac and open the downloaded copy, not
  running a desktop app on the server. Never overwrite an existing download.

Implementation touches more than eight files across file dispatch, editor
views, imports, popups, title bar and, where needed, daemon file metadata/
bounded reads. No new background service or credential is required. Verify
CSV quoting/Unicode/empty files and the byte-limit boundary, R and Python
imports with real installed kernels, Markdown source/preview/save, Shell
highlighting without execution, popup reopen/close order, and both local
and remote opening routes. Native visual checks cover light/dark, empty and
long lists, narrow windows, labelled tick endpoints, and sticky headers.
Source files remain unchanged unless explicitly saved or imported into a
running session; reverting the app changes needs no data migration.

Panes sit 3px below the title bar, 3px apart and 3px from the window's edges;
the gaps are drag handles. The left pane holds the project's name (a button
that lists the machine's projects), its files, its kernel rows (language,
version, files using it, memory, and shutting down with an inline
confirmation) and the machine's CPU, memory, GPU and disk (`system_stats`).

**Frosted glass, macOS's colours (Huan, 2026-11-05).** The window is one
sheet of frosted glass and the panes float on it, as in Xcode 27. On a Mac
the system blurs what is behind the window (GPUI's `Blurred` window
background, an `NSVisualEffectView` with its tint taken out), and a neutral
grey tint goes over the blur (`look::wall`); elsewhere the tint is solid.
The editor and diffs are solid panes, so code reads as well as in Xcode;
the side panels (the left groups and the AI pane), the Console, Terminal and
Jobs under the editor (Huan, 2026-11-05) and the Start screen's machine
cards are translucent, so the glass shows
through them. gpui-kit and gpui-cn each paint their theme's background under
the whole window, so both are transparent. There is no warm tint left: the
greys are AppKit's (label, secondary and tertiary label, separator, text
background), read from the system in light and dark; semantic colours are
macOS's red, green, orange and blue (the accessible ones in light); the
accent is the one picked in System Settings ▸ Appearance, read when the
window opens and each time it comes to the front, with the selection and
soft accent made from it; code is coloured as in Xcode 27.2's Default
(Light) and Default (Dark) themes, keywords in bold and comments upright,
on Xcode's editor backgrounds (white, #1F1F24); the terminal's 16 colours
are macOS's system colours. The orange accent of the Tauri design stays only
in the app icon.

One corner radius, 12px, for every rounded thing. All text is 13px, the macOS
standard size, except the kernel rows, resource rows, Git pane and pop-ups at
11px, and nothing is below 10px. Chrome is in the system font. Code, outputs,
Variables, the console and the agent terminals are in Maple Mono NF, the Nerd
Font build of Maple Mono, bundled with the app (OFL) so every Mac shows the
same font and shell prompts can show their icons; the Data Explorer uses the
system font. Code ligatures are on, so `<-` shows as an arrow while the file
keeps what was typed. Plots stay white in dark mode. The panes themselves
don't blur: behind them is already the window's blur, so a second one would
change nothing you can see and cost a window-sized compositing layer per
pane. Styles are in `app/src/look.rs`, which also draws the icons from
`docs/design/ui/icons`.

**GPUI dependency upgrade (Huan, 2026-11-06; 0.9.1 on 2026-11-13).** Kit,
Base, Component, assets and WebView use the latest released 0.9.1 stack; GPUI
Fast is 0.1.15 and GPUI Pre is 0.3.12. `gpui-cn`'s latest release, 0.8.0, still targets the
older Kit. Its source and licenses are retained in `app/vendor/gpui-cn`,
with only the dependency pins and new non-exhaustive theme construction
adapted. This keeps the Ghostty terminal without loading two Kit versions.

**Icon sources (Huan, 2026-11-06).** The 150 shared icons use Gruvbox Material
Icons for files/folders, Lucide for controls, and Developer Icons for available
integration/runtime logos. Atom Material is removed. The shared offline
generator normalizes pinned, vendored SVGs; source mappings, license notices
and generic substitutions are recorded in `docs/design/ui/icons/`. Existing
IDs and file/folder association rules are unchanged. Interface icons follow
text colour; Gruvbox artwork keeps its original colours without contrast
adjustments. Developer logos retain their light backplates. Mac bundles include
the upstream notices. Sandhill's crane, app icon and fonts remain unchanged.
Agent marks have no background (Huan, 2026-11-12): OpenAI's logo for Codex and
Synthetic Sciences' mark for OpenScience, both one colour and drawn in the text
colour; Posit's blue and orange mark (as on Posit Assistant's site) without the
wordmark; Oh My Pi's gradient π without its dark tile. Antigravity uses Huan's
supplied full-colour SVG. The Antigravity, Synthetic Sciences and Posit marks
have no redistribution licence; it must be established before a public release,
as library licenses do not cover them.

Kernel pickers, in the editor and the Console, show the language icon and
version, the number only ("R 4.6.1", "Python 3.12", though Ark says "R version
4.6.1 (2026-06-24)"); their drop-down holds the kernel, environment, machine, the files
using the session, its memory and the session switch, so session IDs appear
only there. The Agents pane came with step 4 (section 5). Thinking effort
now uses Kit's standard stepped slider, with no explanatory text or custom
knob. The model menu stays 380×360px, with a fixed 32px icon-only left provider
rail sorted alphabetically (Favorites first), names in hover tooltips and a
scrolling right model list; plan usage
shows as rings. The right pane (⌥⌘B) came with step 5
(section 5b), one pane per build: Variables first, then Plots (below
Variables), the Data Explorer, Help and Packages, and Git.

---

## 4. Inline outputs that are saved and reappear

The requirement: you run a chunk, see output under it, close everything, reopen days later and see the same outputs. Each format handles this differently.

### Quarto `.qmd` / R Markdown `.Rmd`: sidecar output store
`.qmd` is plain text and has no place for outputs. Keep it pristine, which is good for git and for `quarto render`, and save outputs next to it:

```
analysis.qmd
.analysis.qmd.out/        # or one project-level .labide/ directory
  index.json              # chunk id → {code_hash, exec_count, ts, output refs}
  blobs/sha256/…          # PNG/SVG plots, HTML widgets, data previews
```

- **Chunk identity:** use the chunk `label` when present, otherwise its ordinal position. Store a hash of the chunk's code. On reopen, outputs whose code hash no longer matches are shown greyed out as **stale** rather than hidden.
- **Where:** keep the cache next to the file on the server, so it travels with the project and other machines see it. The Mac also keeps a local copy so reopening is instant even offline.
- **Precedent:** this is how RStudio's chunk output cache and Positron's new Quarto inline output work. Positron "caches Quarto cell outputs separately from the document in its global state directory" ([Positron docs](https://positron.posit.co/quarto-inline-output)). Positron puts the cache in global app state; putting it beside the file is better for a server workflow.
- **Optional:** for a format where outputs live *inside* the file, Quarto also renders `.ipynb` sources with stored outputs, so offer "convert to `.ipynb`" for people who want that.

### Jupyter `.ipynb`: first-class
- Outputs are stored in the file natively, so your existing notebooks open with their outputs exactly as saved.
- Write `.ipynb` back in the standard nbformat 4 layout so Jupyter, VS Code, Positron and GitHub keep reading it. Keep cell ids and metadata you don't understand untouched.
- Render outputs with the same components as Quarto, so there is one renderer for both formats.
- Run cells through the same daemon and kernels as Quarto. A notebook keeps running on the server after the Mac app closes, and outputs produced while you were away are written into the notebook when you reconnect.

### Opening your existing Quarto files
Old `.qmd` files have no saved inline outputs (Quarto never stored them; only rendered HTML or a `_freeze/` folder holds results). They open with code only, and outputs appear as you run chunks. From then on, the sidecar keeps them.

### Sharing with collaborators
Sidecar outputs don't travel if you send only the `.qmd`. Add a **Share** action that writes one self-contained file from the outputs already on screen, without re-running anything:
- `.ipynb`: editable and runnable, and opens almost anywhere.
- Self-contained HTML: a read-only report that opens in any browser.

`.ipynb` files need no export, since they already carry their outputs.

### Rendering outputs fast and safely
- Every MIME bundle goes through one renderer. Plain text, images and markdown are drawn directly. `text/html` and widgets go into sandboxed `<iframe srcdoc>`s that are lazily mounted only when scrolled into view. This is what keeps a 300-chunk scRNA-seq report light.
- Large data frames: send only the first page plus the schema. Fetch more rows on demand through the kernel comm (Ark has a data explorer comm; for Python, write a small comm or use a pandas/polars preview).
- R plots: Ark's plot comm or `httpgd` (SVG) gives you plots that resize instead of fixed PNGs.

---

## 5. AI agents: subscriptions and API keys

This is the part with real rules attached. As of today:

### Claude (Pro/Max)
Anthropic's [Claude Code legal page](https://code.claude.com/docs/en/legal-and-compliance#authentication-and-credential-use) says:

- Subscription OAuth is for "ordinary use of Claude Code and other native Anthropic applications".
- "Developers building products or services that interact with Claude's capabilities, including those using the Agent SDK, should use API key authentication… Anthropic does not permit third-party developers to offer Claude.ai login into their own applications, or to route requests through Free, Pro, or Max plan credentials." Developers also "may not collect, store, or intermediate Claude.ai credentials or session tokens".
- But it does **not** "prevent an end user from signing in to the unmodified Claude Code binary with their own Claude subscription, including where a platform hosts Claude Code".

**What that means for your design:**

| Allowed with your Max/Pro plan | Not allowed with your plan (needs an API key) |
|---|---|
| Your app launches the **unmodified `claude` CLI** (on the server, in a PTY the daemon keeps alive), and you log in through Claude Code's own `/login` flow. | Reading Claude Code's OAuth token and calling the API yourself. |
| Giving Claude Code your daemon's **MCP server** (`.mcp.json`) so it can run chunks, read outputs and inspect variables. That is ordinary Claude Code configuration. | Building your own chat agent on the **Agent SDK** and pointing it at subscription credentials. |
| Your app watching files Claude edits and showing diffs and refreshed outputs. | Shipping the app to other people with a "Sign in with Claude" button of your own. |

Grey area: driving `claude` through an ACP adapter (Zed's "Claude Agent", marimo's agent panel) gives a nicer chat UI. But that adapter is built on the Agent SDK, which the policy names. My reading is that the conservative choice is the real `claude` CLI in a terminal pane. Claude Code's terminal UI already handles plans, diffs, permissions and `--resume`, so you lose little. If you want a native chat panel for Claude, use an API key for that path.

### ChatGPT (Plus/Pro)
OpenAI is more open:
- **Codex CLI** signs in with ChatGPT natively, and `codex app-server` is a documented JSON-RPC interface for embedding Codex in other apps ([OpenAI: Unlocking the Codex harness](https://openai.com/index/unlocking-the-codex-harness)). JetBrains already ships Codex this way ([JetBrains blog](https://blog.jetbrains.com/ai/2026/01/codex-in-jetbrains-ides/)).
- OpenAI also runs **Sign in with ChatGPT** for third-party apps. "ChatGPT plan usage is available to all open-source partners and selected private clients", and requests count against your plan's Codex usage with a per-app cap you set ([quickstart](https://developers.openai.com/siwc/quickstart), [app-server with a ChatGPT plan](https://developers.openai.com/siwc/token-sharing-open-source/codex-app-server), [help article](https://help.openai.com/articles/20001542)).

**What that means for your design:** a native chat panel for ChatGPT is fine. The daemon spawns `codex app-server` on the server, you log in once with `codex login` (device-code flow works on a headless server), and your UI speaks app-server's JSON-RPC. If you ever open-source the IDE, you can register it as a Sign in with ChatGPT client.

### Google AI (Antigravity, 2026-10-25)
Google is the least settled:
- **The Antigravity CLI (`agy`)** provides headless input and output. Sandhill selects both with `--input-format stream-json --output-format stream-json`, writes one user NDJSON message and closes stdin; `--conversation` continues a conversation. The installed agy 1.5.2 also advertises a native `acp` command. Sandhill's current bridge remains headless NDJSON; this fix does not migrate it to ACP.
- **Its terms are unclear for this.** Antigravity's terms forbid "using third party software, tools, or services to access the Service", and Google hasn't answered the forum questions about launching the official `agy` from another app with a personal Google AI plan. Community answers say running the unmodified CLI is fine and extracting its OAuth token isn't.
- **What Sandhill does:** it runs the official, unmodified `agy` in headless mode, as a script would, and never reads its login. Unofficial ACP adapters exist, but some of them call Google's API with the CLI's token, so Sandhill doesn't use them. The safe path is an API key (`GEMINI_API_KEY`, with `modelProvider: "gemini"` in agy's settings), which takes the plan out of it.

### API keys (Anthropic and OpenAI formats)
With an API key, the subscription restrictions above no longer apply, which opens up a native chat panel for Claude too.

- **Same agents, different login.** `claude` accepts `ANTHROPIC_API_KEY` (and `ANTHROPIC_BASE_URL` for Anthropic-compatible endpoints). `codex` accepts `OPENAI_API_KEY` and custom model providers in its config. The daemon starts each agent with the credential you choose per agent in Settings: subscription or API key.
- **One chat panel for everything.** Speak the **Agent Client Protocol (ACP)**, which Zed, JetBrains and marimo also use. Adapters exist for Claude Code (built on the Agent SDK, so use it with an API key) and Codex. Gemini CLI and OpenCode speak ACP natively. With a Claude subscription, the Claude agent stays in the terminal pane, as in the policy section above.
- **Other ACP agents are cheap to add.** For example, [oh-my-pi](https://github.com/can1357/oh-my-pi) (`omp`, MIT) runs as `omp acp`, also has an NDJSON RPC mode (`omp --mode rpc`), and reads MCP servers from `.claude` config. Adding it means one agent entry, an install check on the server (Bun or its prebuilt binary), and pointing it at our MCP tools. Its role here is **open and local models**: Claude Code and Codex are used directly. Local models can run on the server (Ollama, vLLM, llama.cpp) or on the Mac (Ollama/MLX on Apple silicon). In the second case the ssh link adds a reverse tunnel (`ssh -R`) so `omp`, which runs on the server, can reach the model on the Mac.
- **Two wire formats in settings:** "Anthropic Messages" and "OpenAI (Responses / Chat Completions)", each with an optional base URL. This also covers OpenRouter, local models through Ollama, and other providers that copy one of these formats.
- **Optional later: a built-in lightweight agent in the daemon** that calls those two APIs directly (reqwest + SSE in Rust) with the same MCP tools. Good for quick inline actions ("explain this error", "fix this chunk") without starting a full agent. Build it only after the CLI agents work.
- **Key storage.** Keep keys in the macOS Keychain. The agents run on the server and must keep working after the Mac disconnects, so the daemon receives the key at connect time and holds it in memory, passing it only to the agent process. To survive a daemon restart without the Mac connected, optionally store it with `systemd-creds` (encrypted) or in a `0600` file in `~/.config`. Never write it into project folders or logs.

### Common agent design for both
- **Run agents on the server**, not the Mac. They see the data, the conda/renv environments and the live kernels, and they keep working after you close the laptop.
- **One MCP server in the daemon** that both agents use: `list_notebooks`, `run_chunk(file, id)`, `get_outputs(file, id)` (returns text and small images), `list_variables`, `inspect(df, n)`, `start_job`, `tail_job`. This is the piece that makes the agent understand your *running* analysis, not just your files. It is also the piece Positron's assistant and marimo pair have that plain Claude Code lacks.
- **Reading live variables (like Positron's assistant).**
  - *R:* Ark implements Positron's variables and data-explorer comms, so the daemon gets names, types, sizes and previews over the Jupyter protocol, as Positron does.
  - *Python:* ipykernel has no such comm. Use the Jupyter debug protocol's `inspectVariables` (what JupyterLab's debugger uses), or a silent, history-free `execute_request` that returns a JSON summary of the namespace. The silent-execution route works for any kernel.
  - *Snapshots:* a kernel busy with a long cell can't answer, so the daemon stores a variable snapshot after every execution, and agents read that snapshot while the kernel is busy.
  - *Summaries, not data:* send shape, dtypes, a few rows and summary stats. Add bioinformatics summarizers: AnnData (n_obs × n_vars, obs/var columns, layers, obsm/uns keys), Seurat and SingleCellExperiment (assays, reductions, metadata columns), and DESeq2/edgeR results (top genes).
  - *Read-only by default:* `list_variables` and `inspect` never change state. Running code in your session is a separate tool that asks permission.
- Keep agent edits as ordinary file edits. The daemon notices file changes and pushes them to the open editor, and you see a diff and can re-run stale chunks.

### What is built (2026-10-12)
- **Terminals that outlive everything.** Each agent or shell runs under its own small holder process, `sandhilld terminal-host`, in its own process group, like kernels. The holder owns the pseudo-terminal, logs the output to disk (the last 4 MiB are kept) and serves it on a socket in the terminal's folder (`~/.local/state/sandhill/terminals/<id>/`). Closing the app or upgrading the daemon leaves the agent working; the app replays the recent output when it comes back. An agent keeps the tools it started with (its MCP server, hooks and instructions come from the build that started it), so after an update the app marks older agent sessions and offers to start a new one. The protocol is in `docs/protocol.md`.
- **The CLIs stay unmodified.** Sandhill configures Claude Code and Codex only through command-line flags (`--mcp-config`, `--settings`, `--append-system-prompt`, `--model`, `--effort`, `--permission-mode`; Codex's `-c` overrides). It never edits their config files and never reads their credentials; each signs in with its own login flow (Claude Code in its terminal; Codex through app-server's login, which Codex completes and keeps). Flags an older CLI lacks are left out (the daemon checks `--help`). The one exception is Antigravity, whose `agy` reads MCP servers only from its own settings: the first Antigravity conversation adds Sandhill's with `agy mcp add sandhill` (`agy mcp remove sandhill` takes it out). agy starts it as `sandhilld mcp`, which finds the daemon and the project from the environment Sandhill gives agy (`SANDHILL_SOCKET`, `SANDHILL_PROJECT`).
- **Agent state** comes from Claude Code's hooks (prompt, tool, stop, notification) and its status line, which run `sandhilld terminal-event`, and for Codex from what app-server tells Sandhill's bridge. That gives the busy and waiting dots, the "waiting for you" notice, the model, the permission mode (so a mode switched with ⇧Tab shows on the button), the context bar and the plan's 5-hour and weekly use. A status line the user set up themselves keeps working: Sandhill's status line runs it and prints its output.
- **MCP tools, first set:** `list_sessions`, `list_variables` (a quiet, history-free execute with summaries of Seurat, SingleCellExperiment, AnnData and data frames), `run_code` (in the user's session, where results stay) and `get_outputs` (a document's saved outputs, with plots as images). The server is `sandhilld mcp`, which talks to the daemon over its socket. Jobs came later (`list_jobs`, `read_job_log`, 2026-10-29). Not yet: `inspect`, starting jobs, and variable snapshots for busy kernels (listing a busy session says so instead).
- **Code goes into the document (2026-10-12).** Asked to compute or plot something, the agent writes the code into the document with `add_cell` rather than running it out of sight, as Posit Assistant does: below the user's current cell (else after the last cell) in the document the conversation is about (else the one in front), and it runs there, in the document's own session. The output, errors and plots go back to the agent. A failed cell is fixed in place with `edit_cell`. `run_code` stays for quick looks.
- **More MCP tools (2026-11-03):** `get_console` (recent runs in a session, with their code and output), `get_plots` (the session's latest plots as images), `view_table` (a data frame's first rows, also parts like `obj$x` or `adata.obs`), `get_help` (an R or Python help page as text), `list_packages`, and `delete_cell` (notebooks only; the window can bring the cell back). The read-only ones are marked so and need no approval.
- **Bioinformatics assistance (approved 2026-11-09; implementation 2026-11-10).**
  Retain the four general CLIs and one shared MCP server. Four embedded
  standard-format guides cover study design, omicverse-first Python scRNA,
  Seurat scRNA and results review; all initially await human domain review
  and none is a reviewed default recommendation. `get_bio_guide` is bounded
  allowlisted discovery. `begin_analysis`, `check_analysis`, `get_analysis`
  share the existing daemon protocol/journal, not another runtime/database.
  Bind explicit project/session/object/counts provenance and current digests.
  Fixed read-only kernel checks use atomic idle-only actor execution and
  retain actual run/code evidence; final checks reopen saved objects,
  recalculate cited expression summaries and repeat recorded QC constraints.
  Managed exports are readable copies, never authoritative pass input.
  Historical reads are not current validation. Ask/Plan only read these
  guides/history; independent CLI shell access is not a sandbox.
  Human review remains necessary for design, QC, annotations and conclusions.
  See [the full contract and acceptance limits](design/bioinformatics-assistance-plan.md).
- **Skills (2026-11-16).** Skills the user installs (Welcome → Skills) live in the daemon's state folder, `skills/<name>/` with `skills/index.json`, one set per device. Every agent reaches them through one read-only MCP tool, `get_skill`, and the "/" list, never through the agents' own skill folders, so one install works for all seven agents. Installing always goes through a preview the user confirms (source, licence, files, scripts); Sandhill's Suggested list is pinned commits of openly licensed skills, fetched only on Add. See [the plan](design/done/skills-plan.md) and `docs/protocol.md`.
- **Notebook AI (2026-11-03, batch 3 of the Positron-style notebook):** a cell that ends in an error has Fix and Explain buttons, which send a message to the AI pane naming the cell with #. A cell named with # brings its latest output text to the agent (and says how many plots it drew; `get_outputs` shows them). An agent's change to a cell shows as a diff under it (removed lines red, added green) with Keep and Undo; keeping is the default, so ignoring it keeps the change. A cell an agent deletes leaves a "Deleted by Claude" line with Undo (`.ipynb` only). The notebook scrolls to, and briefly lights up, cells the agent adds or changes; a switch in the AI pane's message box turns that off. The tools send `by` (the agent's name) with `add_cell`, `edit_cell` and `delete_cell`.
  - *Ghost cells (2026-11-04).* With the notebook bar's "Suggest" switch on (off at first, as each suggestion runs the agent once), a cell the user ran that ends well gets a suggested next cell under it, faded, with Accept, Accept and Run, and ×. The daemon asks the AI pane's agent (Claude Code or Codex; Antigravity can't answer once from the command line, so Claude Code or Codex stands in) as its own CLI, run once with no tools and with Claude's quick model; it sees the last cells and what the cell showed. Run All asks only after the last cell, and a cell an agent ran gets none. `.ipynb` only, as it adds a cell.
  - *Where the user is.* Each window tells the daemon its document in front, current cell, line and selection, and which documents it has open (`set_focus`). The daemon keeps the latest place per project, also with the Mac closed. Agents read it with `get_focus` when the user says "this cell". Messages no longer carry it (Huan, 2026-11-12): only what the user adds with @ and # goes with a message.
  - *Who edits.* When a window has the document open, the daemon asks that window to make the edit (`add_cell` request, answered with `answer`): it brings the tab forward, inserts the cell as typing would (one ⌘Z undoes it), moves the cursor into it, saves, and runs it as the Run button does. Editing under an open editor would instead race the user's unsaved changes. When no window has it open, the daemon edits the file and the tool runs the cell in the session that last ran the document's cells, else one in the document's folder with the kernel the document asks for. The chunk ids are the app's (label, else `chunk-<n>`); the daemon finds chunks with the same rules (`crates/daemon/src/qmd.rs`, which the app uses too, with its cases in `tests/fixtures/chunks.json`).
  - *Order.* The project's place moves to each new cell, so a series of cells reads top to bottom.
- **The app** has the Agents pane, a Terminal tab in the Console, and reloads open documents when their file changes on disk (checked every 1.5 s; unsaved changes are never replaced without asking).
- **The chat pane (draft 18, 2026-10-12).** Claude Code shows as a chat, after Posit Assistant, while the unmodified `claude` still runs in its terminal underneath; nothing goes through the Agent SDK or Claude Code's credentials, so it stays on the allowed side of the policy above.
  - *Reading.* Claude Code's hooks record the conversation (`chat.jsonl` next to the terminal's output): the user's messages with where they were, each tool call and its result, the reply text, and what Claude asks. The app draws it as messages, step rows that open into code, diffs and outputs (with plots), and a footer per turn. Like the terminal output, it is kept by the daemon and replayed when the app comes back.
  - *Answering.* Claude Code's `PermissionRequest` hook waits for the app's answer (`answer_terminal`), so approvals, questions (`AskUserQuestion`) and plans are cards in the chat: Allow once, for this chat, or for this project (Claude Code's own rule suggestions); question options; carry out a plan or keep planning.
  - *Trusting the folder (2026-11-04).* The first time Claude Code runs in a folder, its terminal asks whether the user trusts the files there, with "No, exit" picked. A message typed at it from the chat answered that with Enter, and the conversation ended. The app now reads the question off the terminal's screen and asks it as a card in the chat (Yes moves the pick to "Yes, I trust this folder" and presses Enter; No presses Esc), and holds messages until it is answered. Claude Code remembers the answer itself; Sandhill doesn't write its settings.
  - *Writing.* The message box types into the terminal (a bracketed paste, then Enter). The first message is passed on the command line, so a new conversation starts with it. The model and effort go through `/model` and `/effort`, the mode through ⇧Tab (read back from the screen), and stopping is Esc. The terminal is one click away (gear menu), for logins, slash commands and anything the chat doesn't show.
  - *Not yet:* switching a running Claude conversation to YOLO, the Restricted and Ask modes, renaming and exporting conversations, and resuming an ended one.
- **Codex in the chat (2026-10-17).** Codex runs as `codex app-server`, its documented JSON-RPC interface for other apps, under a small bridge in the terminal (`sandhilld codex-chat`). The bridge speaks app-server's protocol and writes the same conversation entries and reports Claude Code's hooks write, so the app shows both agents with one chat. In Chat mode the terminal shows a transcript; command-line mode hands it to Codex's native TUI.
  - *Steps.* Commands, file edits (`apply_patch`, shown as diffs), Sandhill's MCP tools, web searches, Codex's thinking summaries and its plan (as a to-do list) are step rows.
  - *Approvals.* Codex's requests (a command, an edit, an MCP tool call, more access, a question) are cards; each names its step. The buttons are the ones Codex offers: once, for this chat (`acceptForSession`), or for this project (a command prefix rule Codex saves).
  - *Writing.* The app sends what the user does as commands (`tell_terminal`): a message (queued while a turn runs), stop, compact, and the model, effort and mode, which apply from the next turn. Modes (Codex's own names since 2026-11-16) map onto app-server's settings per turn: Read Only asks before leaving a read-only sandbox, Default (where it starts) writes in the project and asks for more, Approve for me lets Codex's reviewer decide, Full Access has no sandbox and never asks, Plan mode uses Codex's plan mode and then asks to carry the plan out (yes switches to Default). The model menu lists the models Codex offers, with each one's effort levels.
  - *Signing in.* When Codex needs a ChatGPT login and has none, the chat offers app-server's sign-in: in the browser (on this Mac), or with a code on any device (on a server).
- **Native command-line mode (Huan, 2026-11-11).** Codex and Antigravity
  pause and release their chat bridges before starting their native TUIs in
  the same controlling PTY: `codex resume <thread>` and
  `agy --conversation <id>`, with the same working directory, model,
  approval/permission settings and Sandhill MCP configuration. Returning to
  Chat cleanly quits the TUI (with bounded signal fallback), waits for exit,
  resumes the bridge and imports new turns from the agent's own history API,
  never by scraping its screen. A TUI exit also returns to Chat. The bridge
  and TUI never drive the same conversation together. Switching has a short
  status, and Terminal mode receives keyboard focus. Claude Code and Posit
  Assistant keep their existing behavior.
- **Antigravity in the chat (2026-10-25, corrected 2026-11-06).** Its bridge (`sandhilld agy-chat`) runs the official `agy` in headless mode for each message: one user NDJSON message on stdin, agy's NDJSON events turned into the same conversation entries as the other agents', and `--conversation` to go on (also after a restart). Both input and output formats must be selected; output formatting alone left agy trying to open its interactive TUI. It is unofficial in that Google hasn't said this use is fine; see "Google AI" above.
  - *Steps.* Tool steps show as agy names them, except commands (`run_command`, shown as Bash), new files (`write_to_file`, shown as Write) and Sandhill's MCP tools. The reply streams from agy's text pieces.
  - *Modes.* Headless agy can't stop to ask. Since 2026-11-16 the menu has agy's own five: Default (it edits files in the project, and turns down what it would have asked about, commands by default; the step shows as failed and the chat says to switch modes or allow it in agy's settings), Accept edits (`--mode accept-edits`), Auto (`--mode auto`), Plan (`--mode plan`, then the plan is put to the user as Claude Code's is; yes switches to Default and carries it out) and Skip permissions (`--dangerously-skip-permissions`).
  - *Models (updated 2026-11-08).* The menu reads `agy models`, defaults new
    conversations to its first returned actual ID, and groups returned sibling
    effort variants into one row. A Gemini Flash Low/Medium/High slider selects
    its exact `-low`/`-medium`/`-high` ID and sends no contradictory separate
    `--effort`. Nonvariant models retain CLI effort behavior. Explicit selections,
    raw favorite IDs and existing conversations survive background refresh;
    browsing another provider is not a selection. Stop sends agy SIGINT, then
    SIGKILL after 3 seconds.
  - *Signing in.* agy signs in on its own: when it isn't, the chat shows its error and says to run `agy` once in a terminal on that machine. Plan usage isn't shown, as agy doesn't report it.
  - *Verified 2026-11-06.* The integration test (`tests/antigravity.rs`) uses a strict CLI stand-in, Sandhill's real MCP server and Python kernel, including reply, stop, resume and failure paths. The repaired bridge was also checked against official agy 1.5.2: a single authorized no-tool prompt returned `OK` and ended idle; a locally invalid model produced a visible chat error without a model request.
- **OpenScience in the chat (Huan, 2026-11-11).** Run the unmodified
  `openscience acp` on the daemon's machine through the generic
  `sandhilld acp-chat` bridge, kept alive by the existing terminal holder.
  This integration is chat only, with no native command-line mode.
  Process-local `OPENSCIENCE_CONFIG_CONTENT` denies its built-in `python`
  and `r` tools and adds an absolute instructions-file path under that
  terminal's own state directory, not a project or user configuration file.
  The instructions direct analysis code to Sandhill's live kernels through
  its MCP tools, retaining the existing document/Console routing.
  OpenScience 2.0 ignores MCP servers passed through ACP and runs local MCP
  servers in a sandbox without sockets, so the bridge serves Sandhill's
  tools over loopback HTTP with a per-conversation bearer token and names
  that endpoint in the injected configuration; leave the user's OpenScience
  configuration and login files alone.
  OpenScience governs its own tools and approval options. Sandhill's
  Restricted/Ask/Plan guards independently govern Sandhill MCP operations;
  these guards and the instructions do not sandbox OpenScience's shell.
  Replies, thoughts, tool steps and plans use the shared chat journal, and
  approval cards preserve exactly the option IDs and labels ACP supplies.
  Resume with `session/load` and replay OpenScience's own history, without
  scraping terminal output. Models and variants come from its configured
  model list and live ACP session model state; model changes use ACP's
  supported `session/set_model` extension. Map native modes only from the
  advertised available modes, not assumed OpenCode names. Show no
  subscription quota. A remote machine needs its own installed and signed-in
  CLI. These protocol details were researched from
  [OpenScience's ACP source](https://github.com/synthetic-sciences/openscience/blob/8e3341a5a4d14541623305bb8e602a4fc265ff49/backend/cli/src/acp/agent.ts),
  [configuration merge](https://github.com/synthetic-sciences/openscience/blob/8e3341a5a4d14541623305bb8e602a4fc265ff49/backend/cli/src/config/config.ts)
  and [instruction loading](https://github.com/synthetic-sciences/openscience/blob/8e3341a5a4d14541623305bb8e602a4fc265ff49/backend/cli/src/session/instruction.ts).
  *Verified 2026-11-11* with openscience 2.0.153 and Huan's ChatGPT (Codex)
  sign-in (`openai-codex/gpt-5.6-luna`): the agent listed sessions, asked
  for `run_code` through a chat approval card, ran it in the live Python
  session (Console "run by OpenScience", Variables updated) and answered 42.
  OpenScience's "Ask always" project mode asks even for read-only tools.
- **Oh My Pi in the chat and command line (Huan, 2026-11-12; plan in
  [design/done/pi-omp-plan.md](design/done/pi-omp-plan.md)).** Run the unmodified
  `omp acp` through the same `sandhilld acp-chat` bridge. Sandhill's tools
  reach it from the bridge's loopback HTTP MCP server, named in the ACP
  session request (omp 18 accepts `http` servers there), so no omp file is
  edited. A per-run `PI_CONFIG_FILES` overlay in the terminal's state
  directory sets `tools.approvalMode: always-ask`, so omp asks before steps
  that write or run (its own default approves everything); Full access and
  Edit mode answer those requests in the bridge, the rest become chat
  approval cards. omp has no system prompt over ACP, so a new session's
  first message carries Sandhill's instructions. Models and thinking effort
  come from omp's `configOptions` (model and `thought_level` selects, set
  with `session/set_config_option`; omp has no `session/set_model`); before
  a conversation, `omp models --json` with its default model role
  (`modelRoles.default`, which may carry `:effort`) and
  `defaultThinkingLevel`. Same-named models from several providers are told
  apart by provider. Resume uses `session/load`. Command-line mode quits
  `omp acp` (one process owns a session) and runs `omp --resume ID` in the
  terminal, with `--approval-mode` from Sandhill's mode, the instructions
  and a per-run extension (`omp -e`) offering Sandhill's tools from the
  same loopback server, since omp's TUI reads MCP servers only from its
  `mcp.json` files; back in the chat the bridge loads the session again,
  importing what was said there. Show no quota. Logo: the official mark
  from the oh-my-pi repository (MIT). *Checked 2026-11-12* against omp
  18.18.9 without a model turn: initialize, `session/new` with Sandhill's
  HTTP MCP server (omp connected and listed tools), `session/load`,
  config-option model/thinking changes, `set_mode`, the approval overlay,
  and the TUI extension's tools reaching a local stand-in model endpoint.
  `tests/omp.rs` repeats the protocol checks with the real `omp` in a
  temporary HOME (skipped without it); `tests/omp_native.rs` drives a
  stand-in through a chat turn, a Full-access approval and the handoff to
  the command line and back. A real chat turn, approval card, Sandhill tool
  call and the real TUI handoff still need checking natively.
- **pi in the chat and command line (Huan, 2026-11-12, option A of the
  plan).** pi has no ACP; `sandhilld pi-chat` drives `pi --mode rpc` and
  turns its events into the same chat entries, with pi's model list
  (`pi --list-models`) and thinking levels, resume by session, approvals and
  Sandhill's tools through a per-run pi extension (no `~/.pi` file is
  edited), and pi's own screen (`pi --session ID`) in command-line mode.
  Show no quota. Logo: pi.dev's mark (MIT).
- **Posit Assistant in its own screen (Huan, 2026-11-06).** Posit Assistant is the fourth agent: its terminal app, `pa` (install with `curl -fsSL https://assistant.posit.co/builds/install-tui.sh | bash`), runs in the AI pane as its own screen, not as a chat. What was looked at first: `pa` has no protocol for editors and no machine-readable output (its one-shot mode prints text), and no way to open its web interface on its own; the RStudio and Positron versions get theirs from Posit's own extension, which is under Posit's licence (an EULA), so Sandhill doesn't take it apart.
  - *Sandhill's tools.* `pa` reads administrator defaults from `POSIT_ASSISTANT_SETTINGS_DEFAULT`, below the user's own settings, so Sandhill adds its MCP server there (`{"mcpServers": {"sandhill": {"type": "local", "command": [sandhilld, …, "mcp"]}}}`) and leaves `~/.posit/assistant/settings.json` alone. Defaults the daemon's environment already sets are kept. `pa` passes its environment to the server, so the server finds the conversation's terminal, and the Console marks its runs "Posit Assistant". Checked against `pa` 0.1.0: it connected and ran `list_sessions`.
  - *The first message.* `pa` with a message answers once and quits, so the daemon starts it bare and the app types the message (with what was added with @ and #, written out as the daemon tells the other agents) once the screen shows its empty box ("Enter a prompt") and isn't asking whether to trust the folder: keys typed then would answer that question. It goes as one paste, then Enter 300 ms later (`pa` dropped an Enter that came 60 ms after the paste). Messages from Fix and Explain go the same way.
  - *What the pane shows.* No chat and no chat/terminal switch; the message box shows only for a new conversation. Its provider picker remains available to switch agents (2026-11-08), but Sandhill's model, effort, mode, web and context controls stay hidden because `pa` has its own menus. An ended conversation offers "Start a new conversation", as `pa`'s screen has no way to resume one from the command line. Its icon is the black mark from Huan's Posit Assistant reference image.
  - *Tested with a stand-in.* `tests/posit.rs` drives a stand-in that reads the defaults as `pa` does and adds a cell through Sandhill's real MCP server and a real Python kernel; the real `pa` needs a model provider's sign-in.
- **The message box (drafts 30–35, 2026-10-25).** What goes with each message is chosen in the box and reaches the agent the same way for both agents: the app writes the choices into the conversation's report (`agent.json`, through a `set` command), and the prompt hook (Claude Code) or the bridge (Codex) reads them when the message is sent and adds a few lines to it.
  - *@ picks.* Typing @ lists the project's files (`list_files`), the front document's chunks, the sessions' variables and the recent plots. A pick is a chip; pasted, dropped and attached files are saved in `.sandhill/attachments/` and become chips too, as do plots (saved as images there so the agent can read them). The chips go with the next message only.
  - *The file the agent works in.* Conversations are about the whole project, not one file (Huan's call, 2026-10-31; until then a chip by the model followed the front document or was pinned to one). Each message works in the first document it names with @, and `add_cell` writes there; a file counts for the message that names it only, and with none named `add_cell` runs the code in the Console (Huan's call, 2026-11-01). Once a notebook is named, # picks one of its cells by number. The message records which file it was, so a long turn keeps writing in the file it started in. The box shows no @ of its own: typing @ opens the menu.
  - *Models before the first message.* A new conversation's model menu shows each driven agent's real list before anything is sent, never by spending a model turn: the daemon's `agent_models` asks a short-lived `codex app-server` (`model/list`), runs `agy models`, `openscience models --flat`, `omp models --json` (with `omp config get` for its default model and thinking level) or `pi --list-models`. Once a conversation runs, its own list replaces it.
  - *Web.* No switch since 2026-11-16: each agent keeps its own web-search default (the globe only ever stopped Claude Code; the others were only asked). The eye that stopped the app sending where the user is went when messages stopped carrying it (2026-11-12).
  - *"/" commands and skills (2026-11-16).* The daemon's `agent_commands {kind, cwd}` returns `{kind, name, description, path?, prompt?}` items ending with Sandhill's four guides. Claude Code's come from the project's and the user's `.claude/commands/**/*.md` and `.claude/skills/*/SKILL.md` (project names win) plus `init`, `review` and `security-review`; Antigravity's from the project's `.agents/skills` and `~/.gemini/config/skills` (the roots the installed `agy` names); scans skip symlinks and unreadable files and are bounded (eight levels, 2,048 entries per root, 64 KiB per file). Codex is asked with `skills/list`, pi with `get_commands` (no session saved); each probe stops within five seconds, and a failure leaves the guides. OpenScience and Oh My Pi put their ACP commands in the conversation's report. A Codex skill chip goes as app-server's `{type: "skill", name, path}` input, never in the refs text.
  - *Models.* The model menu has a tab per agent and a favourites tab (starred models, kept per Mac), with ⌥1–9 to pick. Once a conversation has started, the other agents' tabs are dimmed: a conversation can't change agent.
- **Next:** the context breakdown. OpenScience and Oh My Pi use the
  generic ACP bridge; pi has its own RPC bridge.

**gpui-ai assessment (2026-11-06, not adopted).**
[`gpui-ai` 0.8.0](https://github.com/labcoder/gpui-ai/tree/8a2cbf8b639f207de5b4b1a84cd98299050c1a6a)
is an MIT-licensed, controlled UI component library, not an agent runtime.
It provides streamed Markdown, tool/approval cards, a virtualized chat and
a prompt bar; the host owns requests, storage and long-lived state. It can
therefore render Sandhill's existing daemon events without replacing its
CLI bridges, remote protocol or journal. It does not turn `pa`'s terminal
screen into a structured chat or expose Posit's embedded web interface.
It is not a drop-in dependency: that revision targets `gpui-component` and
`gpui-base` 0.6, while Sandhill now uses GPUI Kit 0.9.0 with `gpui-fast`.
The supplied `ChatMessage` model has no custom element slot for Sandhill's
interactive tool steps and approval cards; those would need a custom list
composed from the independent `ToolCall` / `ApprovalCard` components.
A future adoption needs dependency/renderer adaptation, event mapping for
Sandhill's approval scopes and questions, and retention of its `@`/`#`
context and terminal branch. Its styles are overridable; adopting it need
not replace Sandhill's macOS palette. Recommendation: consider individual
message/tool components first, not a wholesale panel replacement. This is
a source-level assessment only; no integration build or migration was done.

### Verified bioinformatics assistance (proposal, 2026-11-09)

Huan chose existing general-purpose agents plus reviewed bioinformatics
guides, tools and executable checks, with analytical accuracy first.
The [implementation proposal](design/bioinformatics-assistance-plan.md)
defines shared MCP delivery, study-design assistance and Python/R
single-cell workflows, daemon-owned evidence records, scientific-review
boundaries and acceptance criteria. Implementation is not yet approved.
This does not adopt a separate specialist agent, replace existing kernels
or make CLI execution a sandbox.

---

## 5b. Features to adopt from Positron (added 2026-10-11)

Positron is the target experience. The plan is to keep its features and drop its weight. Most of its RAM and CPU comes from the platform, not the features: Electron, the VS Code extension host (a Node process) and an extension for every language. Here every feature is built in, and the panes render only what is on screen.

| Positron feature | Adopt? | How we get it |
|---|---|---|
| **Console + send to console** (Cmd+Enter runs the line or selection from a script, `.qmd` or `.ipynb`; R and Python sessions side by side) | MVP | Daemon sessions + xterm-style console view. One session can serve several files. |
| **Variables pane** | MVP | R: Ark's variables comm. Python: the debug protocol or silent introspection (§5). The same data feeds the agent. |
| **Data Explorer** (sort, filter and column stats on millions of rows, computed in the kernel, not the UI) | MVP | R: Ark's data-explorer comm. Python: implement the same comm for pandas/polars/AnnData `.obs`. This is the feature that matters most for big count matrices and metadata tables. |
| **Plots pane** (history, re-render on resize, save/copy) | MVP | R: Ark's plot comm. Python: matplotlib/plotly MIME outputs. |
| **Help pane** (`?fn`, docstrings) | MVP | Ark's help comm for R. Python: `inspect` and pydoc rendering. |
| **Viewer pane** (htmlwidgets, HTML reports, Shiny) | v1 | Built 2026-10-26: see below. Pages run in sandboxed frames; a server's ports are carried over the daemon's own connection, not a second ssh login. |
| **Quarto inline output** | MVP | Already the core of §4. |
| **Interpreter discovery and switching** (conda/mamba, venv, uv, renv, several R versions) | v1 | The daemon scans the server and remembers a choice per project. Started: kernels inside conda environments, pyenv versions and the shell's environment are found without `ipykernel install` and run with their environment activated, including environments that have ipykernel but no `kernel.json` (Jupyter starts those too). |
| **MCP server for coding agents in live sessions** (Positron 2026.10, experimental) | MVP | Already the core of §5. Positron adding the same thing confirms the design. |
| **Assistant aware of your session** (variables, plots, errors) | v1 | The same MCP tools, plus "Fix" and "Explain" buttons on errors (built 2026-10-27: under every error in a cell or the Console, they send the error, the code that raised it and the cell to the conversation in front, or put them in the new conversation's box when no agent is running). |
| **Packages pane** | v1 | Built 2026-10-18: see below. |
| **Run App** (Shiny, Streamlit, Dash…) | later | A job plus the Viewer pane. |
| **Data Connections pane** (databases, warehouses) | skip | Rarely needed for bioinformatics. |
| VS Code extension marketplace | **skip** | This is where most of Positron's memory goes. Build in what you need instead. |

**Built: the shared Console and Variables (step 5, part 1, 2026-10-18).**
- *One Console per session, not per file.* As in RStudio and Positron, the Console shows a session, whatever ran the code: its prompt, a script's ⌘↵ or an agent (marked with its name). Pills in the Console's tab strip switch sessions; the session in front follows the document in front and the script whose line just ran.
- *Notebooks keep their output to themselves (2026-10-24).* By default a notebook or Quarto cell's run stays under the cell: the Console doesn't show it and doesn't switch by itself to a session only notebooks have used, though its pills list every live session and picking one shows it (Variables, Help and Packages still follow the notebook's session). The Console and Plots start minimized (2026-10-25); the Console opens when a script's line runs in it, and Plots when code outside notebooks draws a plot. The Console's R and Python buttons then use the session the notebooks run in rather than starting another. The ☰ menu's "Notebooks and Quarto" choices bring their runs into the Console (echoed in two lines, with the output left under the cell) and their plots into Plots. Both are kept per Mac. Every run is already journaled, so the Console is the session's journal (`session_runs`), not something the app keeps; Clear hides what came before on this Mac only.
- *The prompt* is a small CodeMirror editor: Enter runs the code once it is complete (brackets and strings closed, no trailing operator in R, no open block in Python), otherwise it starts a new line; ⇧Enter always does; ↑/↓ walk the session's history, Ctrl-C interrupts and Ctrl-L clears.
- *Variables* uses neither Ark's comm nor the debug protocol for now: the daemon runs a quiet, history-free snippet in the session (`list_variables`), which works the same for Ark, IRkernel and ipykernel. It knows data frames, factors, lists and dicts, Seurat and SummarizedExperiment objects, and AnnData (`X`, `obs`, `var`, `obsm`, `layers`, `uns`, `raw`) with the columns of `obs` and `var` a click away. The list is read again after each run in the session, never while it is busy. Ticked variables can be deleted (`delete_variables`), which also runs `gc()` / `gc.collect()` and, in Python, drops IPython's references to old outputs, so the memory really comes back; the pane says how much. The agents' `list_variables` tool reads the same list.
- *Agent code in the Console.* Under the modes, "Agent code goes: In the file / In the Console" decides what the agent's `add_cell` does: add a cell to the linked file (the default), or run the code in the document's session, where it shows in the Console as the agent's. The choice can change while the agent runs.

**Built: Plots (step 5, part 2, 2026-10-18).**
- *One history for the project.* The right pane is now two cards, Variables over Plots, with a gap between them to drag; each minimizes and maximizes on its own. Plots shows every image the project's live sessions drew, R and Python together and in time order, whatever ran the code: the Console, a script or an agent, and a notebook or Quarto chunk (labelled "analysis.qmd · umap" when the chunk has a label) when the ☰ menu says so. A new plot comes to the front as it is drawn; the arrows, ← and →, and the thumbnails go back through the rest.
- *From the journal.* Outputs are already journaled, so the pane reads each session's latest 100 images (`session_plots`) when it opens and then follows the sessions' events; nothing is kept by the app. Clear hides the plots so far on this Mac only; they stay in the Console and under their cells. A session's plots go when the session ends, as in RStudio.
- *Zoom, copy and save.* The magnifier opens a plot large. An SVG plot can be saved as SVG, or as a PNG drawn from the SVG at a size you type or pick (4 × 3 in, 6 × 4 in or a 16:9 slide, at 300 dpi, written into the PNG so Word and PowerPoint place it at that size); the plot keeps its shape on a white background. A PNG plot is saved or copied as the kernel drew it. Save asks for a path in the project, `figures/<chunk label or file name>.png` at first and then the folder used last, and asks before replacing a file; the daemon writes the bytes (`write_file` with `base64`). Copy puts a PNG on the clipboard.
- *Re-drawing to save (2026-10-25).* A plot whose session can draw it again is saved at any size, as PNG, SVG or PDF: the session draws it again into the file (`redraw_plot`), so lines and text stay sharp. Ark keeps a recording of each plot it shows (`.ps.graphics.render_plot_from_recording`'s recordings, replayed on a `png()`, `svg()` or `pdf()` device); a Python session keeps its last 10 matplotlib figures (a figure holds its data, and a UMAP of many cells is big), because code run when it starts wraps IPython's figure formatters and tags each image with its figure (`savefig` at the size picked, with the figure's own size put back). The dialog asks when it opens and says why not otherwise ("since only plots R drew can be drawn again"); those plots save as before. Copy still copies the plot as shown.
- *Re-drawing in the pane (2026-10-29).* As in RStudio and Positron, a moment after the pane stops changing size the plot in front is drawn again at the pane's size (`redraw_plot` with `show`, which sends the PNG back instead of writing a file), so text and lines stay sharp instead of stretching. A busy session leaves the plot as it is. IRkernel's plots can be drawn again too (2026-10-31): code run when an IRkernel session starts wraps its `send_plot`, which keeps the recordings of the last 10 plots (`recordPlot()`'s, which IRkernel already makes) and tags each image with `sandhill_plot`, as Python's figures are; older plots, and plain images, show as drawn, never larger and never out of shape.
- *Resizable enlarged preview (2026-11-09, Huan's approved direction).* Drag the zoom card's bottom-right corner to change width and height independently; window resizing also changes its drawing area. Once the measured preview size is stable for 400 ms, use the existing daemon-only `redraw_plot(show: true)` request at that logical size and the display's backing resolution. Keep at most one request in flight per preview, coalesce later sizes, and reject replies for an old size, plot, closed preview or connection. R replays its recorded plot and matplotlib renders its retained Figure; never rerun the analysis or stretch the original image. Preserve explicit fixed-aspect settings (`asp`, `coord_fixed`, `aspect="equal"`). Busy/ended sessions, missing recordings and plain images retain their image with an explanation. Preview geometry is separate from the labelled Export dimensions; changing plots retains the card size, and new live output does not silently replace the enlarged plot.
- *Zoom must settle (2026-11-11).* The preview's status line is a single fixed-height line, so changing messages ("Waiting for the preview size to settle…", "Redrawing preview…") never changes the measured canvas, and sizes within one logical pixel count as unchanged. Only a real size or display-scale change starts the debounced redraw; an idle enlarged plot never moves.

**Built: Data Explorer (step 5, part 3, 2026-10-18).**
- *A table as an editor tab.* The table button on a data frame or matrix in Variables (`seu@meta.data`, `adata.obs`, a plain matrix or NumPy array) opens it as a tab beside the documents, with a table icon and the table's code as its name. The bar over it says the session, the table and whether it is live; Copy as code and Export CSV sit on the right. The status bar says how many rows show ("618 of 2,638 rows"). Table tabs, like diff tabs, come back when the project opens again (2026-10-25), with their sorting and filters, which keep their columns' names so they follow them if the table changed meanwhile. A table follows its session through a restart, and one whose session has ended opens in the project's session for its language when there is just one; with none, or several, it stays closed (2026-10-31).
- *Computed in the kernel, like Positron.* Neither Ark's data-explorer comm nor a comm of our own for now: the daemon runs a quiet snippet in the session for each request (`table_rows`, `table_summary`, `table_export`), the way Variables does, so it works the same in Ark, IRkernel and ipykernel. Filtering and sorting happen in R or pandas, and only a page of 200 rows comes back. The grid draws only the rows and columns on screen and reads more pages as you scroll; a 1,000 × 1,000 array or a table of a million rows scrolls the same as a small one. Columns are read the same way, 100 at a time, cells and stats, so a table of 30,000 columns (genes as columns) shows them all; the column list at the side draws only the names in view (2026-10-29).
- *Columns at the side.* Each column has a 20-bin histogram (numbers) or bars of its commonest values, colored for a factor or category of up to 10 values (the same colors in its cells). The picked column's min, median, mean and max, or its values with counts, show under the list; clicking a value filters to it.
- *Filters, search and sorting (updated 2026-11-11).* The single 29px editor action row contains the icon-only stats toggle, live dataset label, refresh, Copy as code, filter chips, Add filter, search and icon-only CSV export, with pane height controls fixed at the right. Its middle scrolls horizontally when needed. Add filter toggles its popup, with outside-click and Escape dismissal. Search waits 300ms after typing, matches literal text case-insensitively across every column, combines with filters using AND, and runs in the R or Python session before sorting and paging. Copy as code and CSV export include the search. A separate small arrow at the right of a column header sorts ascending, descending, then off; shift-click adds a sort key. Header names and types have a full two-line row.
- *Read-only table selection (2026-11-11).* Data Explorer and notebook output tables use I-beam cursors and live rectangular drag selection, with row-number and column-header tints. Shift-click extends, row numbers select rows, headers select columns, and dragging either selects adjacent rows or columns. Command-A selects all, Escape or empty-space clicks clear, and Command-C copies values as quoted TSV without headers. Selection redraws stay local to the table, never refresh the notebook. Explorer copies fetch unloaded pages and reject more than 1,000,000 selected cells with a message. Alternate neutral row tints work in both themes. The footer shows selection size at the left and filtered row/column counts at the right, without sorting-status prose.
- *Live.* After each run in the session the table is read again, keeping the filters and sorting on their columns by name, so a new column or a changed value shows at once. While the session runs code the last rows stay and a note says so.
- *Copy as code and Export CSV.* Copy as code puts R or pandas code that does what the view does on the clipboard (`x <- meta; x <- x[which(x$percent.mt < 5), , drop = FALSE]; x <- x[order(-xtfrm(x$nCount_RNA)), , drop = FALSE]`). Export CSV writes the rows shown, filtered and sorted, to a file in the project (`write.csv`, `to_csv`).
- *From code (2026-10-25).* `View(df)` in R and `%view df` in Python open the table too, from the Console, a script or a notebook. Ark's own `View()` opens its data-explorer comm; the daemon reads the comm's title and closes it, as there is no Positron to answer it. IRkernel and IPython sessions get a small `View` and `%view` when they start (in an environment of their own, `tools:sandhill`, in R), which send a display_data of Sandhill's own type. Either way the app gets a `sandhill.view` event and opens what was named, which has to be a variable or a part of one (`seu@meta.data`, `adata.obs`); `View(head(df))` says to assign it first.
- *Not yet.* Polars and Arrow tables; editing cells; and keeping the filtered, sorted order between requests, which each request recomputes (fine at a few million rows, slow beyond).

**Built: Help and Packages (step 5, part 4, 2026-10-18).**
- *Tabs on the top card.* Variables, Help and Packages share the right pane's top card as tabs, over Plots. All three follow the session in front, the Console's.
- *Help.* Two boxes, as in the mockup: a function or topic, and a package (empty for all). Matches show as you type, six at first: in R every installed package's topics (titles are searched too, so "principal components" finds `prcomp`), in Python the public functions, classes and modules of the modules already imported. Attached or imported packages come first. A click shows the page under the matches, with Back and Forward. R's page is the package's own HTML from `tools::Rd2HTML`, cleaned of scripts; its links open here, "Run examples" runs `example()` in the Console, and web links open in the browser. Python's page is the signature (one argument to a line when long), the docstring read as sections (numpydoc and Google style, doctests and reStructuredText notes), and a module's or class's public names, each a click away; `:func:` references and See Also names are links. Source shows the function's code: R's deparsed, Python's from its file.
- *Help from the Console.* `?lm`, `?stats::filter`, `??pca`, `help(lm)` and `help(package = "Seurat")` in R, and `?np.mean`, `np.mean?` and `help(np.mean)` in Python, open the Help pane instead of running, and go into the prompt's history. F1 on a name in an editor or the Console (or on a selection) opens its help too.
- *Computed in the kernel, again.* Not Ark's help comm or R's help server: the daemon runs a quiet snippet (`help_topics`, `help_page`, `help_source`), which works the same in Ark, IRkernel and ipykernel and over ssh, with no port to forward. R's index of 70,000 topics is read once per session (under a second) and kept until a library changes; a search then takes about 50 ms. Python's index is kept until more modules are imported.
- *Packages.* What's installed for the session's language: R's libraries, or the Python environment's distributions, attached or imported ones first, then the rest with "loaded" where they are. The filter searches names, then titles. A click opens the package's help: its topics and its overview page in R, its module in Python. Install (the + button) runs `BiocManager::install()` in the Console when BiocManager is there (CRAN and Bioconductor), otherwise `install.packages()`, setting the CRAN mirror if none is set; in Python `%pip install`, or `%conda install` with conda-forge and bioconda in a conda environment. Remove asks first, then runs `remove.packages()` or `%pip uninstall` (`%conda remove` for what conda installed). The list is read again after each run, so `library(DESeq2)` shows it attached.
- *Vignettes (2026-10-25).* A package's vignettes show under its topics in Help, and a search finds them by title too. One opens in the pane, in a frame of its own: an HTML vignette as its package built it, with its styles and pictures but none of its scripts (so math shows as TeX, and tabs don't switch), and a PDF in the PDF viewer. Its links open in the browser, and "R code" shows the vignette's code. The daemon only lists them (from each package's `Meta/vignette.rds`, kept with the help index); the app reads the built file from the session's machine.
- *Python updates (2026-11-04).* The ⇣ button works in Python too: conda's own packages are checked against the environment's channels with a dry run of `conda update --all` (so only versions that fit the environment show), the rest against PyPI. Updating runs `%conda update -y` for conda's and `%pip install -U` for pip's, in the Console.
- *Updates (2026-10-25).* In R, the Packages pane's ⇣ button asks CRAN and, with BiocManager, the matching Bioconductor release which packages have newer versions (`package_updates`). It is a button, not automatic, because the session downloads the repositories' indexes, which takes a few seconds and needs the internet. Each package with one shows "update 1.2.3" (the mockup's), which updates it in the Console the way Install does (`BiocManager::install()` or `install.packages()`); "Update all" does them together. A package updated since drops out of the list.
- *Not yet.* Python updates (pip's check asks PyPI one package at a time, and mixing pip into a conda environment breaks it); help for objects that aren't imported in Python (it searches what's imported, and opens any module by name); and math in R pages and vignettes, which shows as LaTeX.

**Built: Git (step 5, part 5, 2026-10-18).**
- *A tab on the top card.* Git joins Variables, Help and Packages as a tab, and the card grows to two thirds of the pane while it shows. It follows the project's repository, not a session. Everything in it is 11px and never bold, as the spec says.
- *The header.* The branch (a click opens the Branches menu), how far it is ahead of and behind its remote branch, and buttons for Fetch, Pull, Push and Undo the last commit, then the Branches menu (switch, including to a remote branch, which makes the local one, or make a new one), Stash (put every change aside, new files too, and bring the last stash back) and More (stage all, unstage all, throw away all unstaged changes, read again).
- *The graph.* Every branch's commits, newest first, in lanes that never move, so a branch is a straight line; the working tree is the top row, joined to HEAD by a dashed line, with how many files are staged and not. Each commit shows the author's initials on a coloured gradient, or Claude Code's (CC) or Codex's (CX) when the agent wrote it or is a co-author, its message, how long ago, and its branches and tags (a globe when the remote has the branch). More commits load as you scroll. A click on a commit shows its whole message, who wrote it and the files it changed; a click on a file opens what the commit changed in it.
- *Staging and committing.* Under the graph, Staged and Unstaged files with their letter (M, A, D, R, ! for a conflict) and lines added and removed; hovering a file offers Open, Throw away (Delete for a new file, after asking) and Stage or Unstage. A click opens the file's diff as an editor tab, with old and new line numbers, which follows edits and can stage or unstage the file from its bar. A new file over 50 MB shows its size, and staging it asks first: "Ignore it" adds it to `.gitignore` (and stages that), since GitHub refuses files over 100 MB. The summary and description boxes take the message (⌘↩ commits); Amend changes the last commit, filling in its message; the Commit button's menu has Commit and push, and Stage all and commit.
- *Write it.* Asks Claude Code (its small model, no tools) or Codex for the message, from the staged changes and the recent subjects, and fills the boxes to edit. It runs the agent's own CLI as the user is signed in to it, like the Agents pane; no keys or tokens are read.
- *Lines and hunks (2026-10-25).* A diff tab stages part of a file as `git add -p` does. Each hunk's header has Stage and Throw away (Unstage in a staged diff); clicking a changed line's numbers picks it, Shift-click picks the range to it and a drag picks every line it passes, and the bar then offers Stage 3 lines and Throw away (or Unstage 3 lines). A removed line and the added line in the same place pair up, so changing one line of a block picked on its own lands where that line was. Throwing away asks first. The daemon makes a patch of just those lines from the diff the tab shows and has `git apply` (`--cached` to stage, `--reverse` to undo) check it against the file, so an edit since then makes it refuse rather than change the wrong lines. New, deleted, renamed and conflicted files stage whole.
- *Merging (2026-10-25).* The Branches menu's "Merge into main…" lists the other branches, local and remote; picking one asks first, then merges it into the branch checked out as `git merge` would (a fast-forward when it can). When both changed the same lines, git stops and the pane shows the conflicts; otherwise it says what was merged.
- *Stopped in a merge.* While git is merging or rebasing a banner says so with Stop (`merge --abort` and the like); conflicted files show with ! among the unstaged, to fix in the editor and stage. Each one also has Mine and Theirs, which take that side's version of the whole file and stage it (while rebasing, Mine is the branch's own commit, though git calls it "theirs"). While merging, the commit boxes start with git's message ("Merge branch 'exp/harmony'"), so Commit ends the merge even when nothing new is staged; a rebase has Continue in the banner instead, once nothing is left with !.
- *Rebasing (2026-10-29).* The Branches menu's "Rebase main onto…" lists the same branches as Merge; picking one asks first, then makes the branch's own commits again on top of it (`git rebase`), for a straight history. More has "Pull, with my commits on top" (`git pull --rebase`). Both stop on conflicts as above, and Continue goes on to the next commit, with git's message rather than an editor.
- *Squash and reorder (2026-10-31).* More has "Squash or reorder commits…", which lists the branch's commits not pushed yet, newest at the top as in the graph, back to the last merge and at most 50. Each can be dragged (or moved with its arrows) and squashed into the commit below it; the commit others fold into shows their messages joined, to edit. The button says what will happen ("Squash into 2 commits", or "Reorder"). The daemon does it as an interactive rebase whose list it writes itself (`pick`, `fixup`, and an `exec git commit --amend -F` for a new message), with `--autostash` so changes not committed stay. Pushed commits aren't offered, since changing them would need a forced push; a reordering that conflicts stops like any rebase, with Continue and Abort.
- *Tags (2026-10-29).* The Branches menu lists the newest tags under Tags, with New tag…, which tags the current commit with an annotated tag (a release, or the version a paper used). A tag's click offers Push, which sends it to the remote, and Delete, which takes it off here only. Tags already show beside their commits in the graph.
- *Signing in (2026-10-29).* When git needs a username, a password or token, or an ssh key's passphrase that no credential helper or ssh agent has, it asks in the window: git and ssh run `sandhilld` as their askpass program, which passes the prompt to the daemon and the daemon to the window whose Git pane started the fetch, pull or push. What is typed goes straight to git and is not kept; on a Mac git's own keychain helper remembers what worked, as in a terminal. For GitHub and GitLab the box says to use a personal access token, since they refuse account passwords.
- *The git program.* The daemon runs the command line `git` on its machine (`git_status`, `git_log`, `git_show`, `git_diff`, `git_run`, `git_write_message`), so it works the same over ssh, with the user's ssh keys and credential helpers, and a failure shows git's own message. The pane reads the status every 3 s while it shows and when the window comes to the front, and reads the log again only when a branch or tag moves. A repository without a name and email for commits gets a form that sets them globally; a folder that isn't a repository gets a button to start one.
- *Not yet.* Side-by-side diffs and syntax colours in them; diffs that understand notebooks (an `.ipynb` diff is its JSON); adding and changing remotes, and interactive rebasing (squashing, reordering), from the pane (a terminal does these); and avatars from GitHub.

**Built: the Viewer (2026-10-26).**
- *A tab beside Plots.* The right pane's bottom card has two tabs, Plots and Viewer, each with its own buttons; the one in front is kept per Mac, and a new plot or page brings its tab to the front (and the right pane, for a page). The Viewer's bar has Back and Forward through its pages, Reload, Open in browser and Clear, and a line saying where the page in front came from ("Console · datatables", "qc.R · leaflet", "results/multiqc_report.html").
- *Pages from the Console and scripts.* An htmlwidget (DT, plotly, leaflet), `htmltools::html_print()`, a plotly figure from Python, or any HTML output that runs scripts goes to the Viewer when the Console, an agent or a script showed it, and the Console says "datatables in the Viewer", which brings it back to the front. A data frame's table stays in the Console. Notebooks and Quarto documents keep their pages under their cells, as before. The rule is the same in the daemon and the app (`tests/fixtures/pages.json`), and the Viewer reads each session's latest 20 pages from the journal (`session_pages`), like Plots.
- *R's viewer, the same in every R kernel.* A page htmltools hands to R's `viewer` option (`html_print`, a widget printed outside a notebook) needs an IDE to show it, and IRkernel sets none. Each R session now gets a `viewer` option when it starts, which prints the page's path between two escape codes; the daemon takes it out of the output and puts in its place an output of Sandhill's own type naming the page, with the page itself as HTML whose scripts, styles and pictures are put inside it, so it outlives R's temporary folder. The widgets kernels show themselves work as they were, named from the page, or in Ark from its label (`<datatables HTML widget>`).
- *Python plotly.* plotly 7 sends a figure only as data (`application/vnd.plotly.v1+json`), so the app draws it with plotly.js, in the Viewer and under a notebook's cell. plotly.js loads from plotly's own address (cdn.plot.ly), as plotly's own "connected" pages do, so a figure needs the internet.
- *HTML files.* Clicking an `.html` file in the file tree (MultiQC, FastQC, a rendered Quarto document), or Open in Viewer in its menu, shows it in the Viewer with its pictures, scripts and styles: the daemon serves the file's folder over HTTP on its machine's `127.0.0.1` (`serve_page`), at an address signed for that folder so nothing else is served. Reload reads it again, so a report rendered again shows. Files opened are remembered per project on this Mac.
- *A server's pages on this Mac.* The app reaches the daemon's page server, and later apps, through tunnels on the daemon's own connection (`open_tunnel`), as `ssh -L` would but without a second ssh login or a port to agree on: the Mac app listens on a port of its own on `127.0.0.1` for each one, which Open in browser uses too while the app runs. Only the server's `127.0.0.1` is reached.
- *Safe frames.* An output's page runs in a frame with scripts but no origin at all, so it can't reach the app, the daemon or the project's files; a served file or an app runs in a frame of its own address (`127.0.0.1` and a port), which can keep its settings but can't reach the app either.
- *Apps.* A Shiny app run from the Console or a script (`shiny::runApp()`, or a script whose last line is a `shinyApp()`) opens in the Viewer, through a tunnel when the daemon is on a server, and the Viewer's bar gets a Stop button while it runs, which interrupts its run. An address printed in the Terminal on the daemon's machine (Shiny for Python, Streamlit, Dash, `quarto preview`) is a link that opens in the Viewer the same way; other links go to the browser, as before. `browseURL()` of any other page opens it in the browser, and the Console says so.
- *Quarto Preview.* A `.qmd` or `.Rmd` document's bar has Preview (also ⇧⌘K, as in RStudio and Positron, and in ⌘K): it saves the document, then the daemon runs `quarto preview` on its machine in a terminal of its own and the Viewer shows "Rendering report.qmd with Quarto…" until the page answers, then the page, through a tunnel from a server. Quarto renders the document again each time it is saved and the page reloads itself. Stop ends the preview; Preview again starts it again if it stopped. If Quarto fails, the Viewer shows what it said; if Quarto isn't installed, it says how to install it (from quarto.org, or with mamba from conda-forge). Python chunks run in the project's Python kernel's environment (`QUARTO_PYTHON`); R chunks run in the R that Quarto finds, with knitr.
- *R kernels' JSON.* R kernels write JSON from R lists, so an empty map can arrive as `[]` (IRkernel) or `null` (Ark), and IRkernel answers an interrupted run with the status `abort`, which the protocol doesn't have. The protocol crate couldn't read those messages, and one ended the reader: Ark's widgets were lost and an interrupted IRkernel run never finished. The daemon reads kernel sockets itself and mends those first (`quirks`), so an interrupted R run ends with an error as Python's does; a message it still can't read is skipped with a warning.
- *Not yet.* The Viewer uses Safari's engine (WKWebView), which plotly, DT and leaflet support, but it is tried here in Chromium only, and an app's page from `http://127.0.0.1` inside the app's own window is untried there too; links inside a page go in its frame, and the frame's own Back is the page's.

### What code can be reused
- **Ark** (R kernel, LSP and the variables, data-explorer, plots and help comms) and **Air** (R formatter) are **MIT**. Use them freely. Ark alone gives the R side of most of the MVP panes.
- **Positron itself** and **Kallichore** (its Rust kernel supervisor, which resembles our daemon) are under the **Elastic License 2.0** ([licensing](https://positron.posit.co/licensing)). ELv2 allows personal and commercial use and derivative works. The main restriction is offering it to others as a hosted service. Copying ELv2 code puts that part of your project under those terms. Safer path: implement the same comm protocol (Ark's MIT code shows the message shapes) and write our side fresh. Use Kallichore as a design reference.

---

## 5c. Projects (decided 2026-10-11)

RStudio has `.Rproj` and Positron has workspace folders. Sandhill needs the same thing so that one analysis keeps its own interpreters, working directory, tabs and running kernels. Before this decision the app had only a "root folder" box over the file tree, remembered by the Mac app, and kernels started in each file's own folder.

**What a project is.** A project is a folder on one host: the Ubuntu server, or the Mac in local mode. The daemon on that host owns it. A project is marked by one small file that can be committed to git:

```
my-analysis/
  .sandhill/
    project.toml     # shared settings, committed to git
    outputs/         # project-level output cache (gitignored)
    agents/          # agent conversations, to go on elsewhere (gitignored)
    .gitignore       # written by Sandhill, ignores outputs/
  analysis.qmd
  renv.lock / environment.yml / pyproject.toml ...
```

- `project.toml` holds only settings a collaborator should share: the R and Python interpreters (or "detect"), the working-directory rule, and env files to load.
- Personal state (open tabs, pane layout, recent commands) is **not** written into the folder. The daemon keeps it in its own state directory, keyed by the project path. RStudio puts this in `.Rproj.user/`, which people then have to gitignore; we avoid that.
- *Agent conversations are the exception (decided 2026-10-28).* Huan wants a project's chat history bound to the project, so a folder on a portable drive or a synced copy opened on another Mac shows its conversations and goes on with them. They are saved in `.sandhill/agents/`, which git ignores by default (repositories can be public, and chats hold pieces of data, paths and sometimes tokens). Project settings has "Chat history: Kept with the project" (default) or "Kept on this machine only"; switching moves them. Each agent keeps its own copy of a conversation in the home folder (`~/.claude/projects/<path>`, `~/.codex/sessions`), so the folder also carries that copy and Sandhill puts it back before the agent resumes; checked with Claude Code moved to a path with a space, which went on with what it was told. Where Antigravity keeps its conversations isn't known, so on another machine it starts afresh with the chat to read, as any agent does without its copy. Logins never move. Removing a conversation on one Mac removes it on the others too (2026-10-29): the folder keeps a small note that it was removed, without its title or chat, so a Mac that still has it removes it rather than copying it back; one still running elsewhere wins and comes back. Not done: Codex wasn't checked with a real login; the terminal's raw output stays on the machine; a window shows a conversation removed elsewhere until it reconnects. Protocol: docs/protocol.md, "Conversations kept with the project".
- Any folder can be opened as a project. `.sandhill/` is created only when you first change a project setting, so opening a colleague's repo leaves no trace.

**What it does.**
1. **Open and switch.** "Open Project…" picks a folder on a host. A start screen lists recent projects per host (server and Mac separately). A window has a tab per machine, each showing one project; the project name at the top of the left pane switches the tab to another project on that machine (section 3, "The look").
2. **Restore.** Reopening a project brings back its tabs, file tree and layout, and reattaches to that project's kernels that are still running. The daemon tags each kernel session with its project, so sessions can be listed per project.
3. **Interpreter per project.** On first open the daemon looks for `renv.lock`, `environment.yml`, `pixi.toml`, `pyproject.toml`/`uv.lock` and `.venv/`, and proposes the matching R and Python. You confirm once and the choice is saved in `project.toml`. This is the "remembers a choice per project" line in the §5b table.
4. **Working directory.** The console and plain `.R`/`.py` scripts run in the project root, as in RStudio. Chunks in `.qmd`/`.Rmd`/`.ipynb` run in the document's own folder, because that is where `quarto render` runs them, so relative paths behave the same interactively and when rendering (RStudio's default for R Markdown too). `project.toml` can switch documents to the project root.
5. **Startup files.** R starts in the project root, so its `.Rprofile` and `.Renviron` there apply as usual; this is how renv activates. The `.env` files listed in `project.toml` (`.env` unless it says otherwise) are loaded into the environment of the project's kernels, R and Python alike (2026-10-25: R too, as there is no reason to leave it out).
6. **Agents.** Claude Code and Codex start in the project root, so they read the project's own `CLAUDE.md`/`AGENTS.md` and reach the project's kernels through the MCP tools.
7. **Output cache.** Inside a project, the §4 sidecar cache lives in `.sandhill/outputs/` instead of a `.file.qmd.out/` folder per document. Files opened outside any project keep the per-file sidecar.
8. **Sessions shared between files (built 2026-10-11).** Files whose code runs in the same folder share one R and one Python session, as RStudio's scripts share one console: the first time a file runs code it joins the project's session for that language and folder, the one last used in the window, and only starts a kernel when there is none. A notebook that names its kernel joins only sessions of that kernel. "New …" in the kernel bar still gives one file a session of its own, and restarting a session moves every file using it to the new one. Sessions keep running when files and the app close (that is the point of the daemon), so the sidebar lists them with a stop button each, marks the ones no open file uses, and offers "Stop unused", which leaves alone sessions still running code (the daemon reports unfinished runs per session).
9. **Scripts (built 2026-10-11).** `.R` and `.py` files open in an editor with a console under it. ⌘↵ runs the selection, or the statement at the cursor (a call over several lines, a pipe chain, a function or a Python block) and moves to the next; ⇧⌘↵ runs the whole file. Each run is journaled under the file and shows in the Console of its session, which since step 5 is shared by everything that runs code there (section 5b).

**Left out on purpose.**
- Saving the R workspace (`.RData`) on close. RStudio does it by default and most guides say to turn it off; our kernels keep running on the server anyway.
- Reading `.Rproj` files. A folder that has one opens fine; its encoding and working-directory settings can be read later if people ask.
- Multi-root workspaces (VS Code/Positron `.code-workspace`). One folder per project; open a second window instead.

**Build order.** First: daemon calls `open_project` and `list_recent_projects`, sessions tagged by project, the start screen, and restore. Interpreter detection lands with the v1 interpreter-switching feature.

**Built so far (October 2026).** Open and switch, restore and project-tagged sessions:
- The daemon calls `open_project`, `list_recent_projects`, `save_project_state` and `forget_project` (see [protocol.md](protocol.md)). A project's tabs live in the daemon's journal, keyed by its folder, and `start_session` takes the project so sessions can be listed per project.
- The app opens a start screen listing the host's recent projects, with a folder browser that asks the daemon for folders, so it will work the same for a server. Projects open in their machine's tab of the window (until 2026-10-25, each in its own window), and the tabs open at Quit open again at the next launch. Closing the last window with projects shows the start screen again.
- **Quit confirmation (2026-11-08, Huan).** Cmd-Q, the Quit menu, macOS
  termination and closing the last Welcome window ask whether to stop the
  background services. Default No keeps them running; Return/Space choose No,
  Escape or the backdrop cancels. Save edited documents successfully before
  either exit path; save conflicts keep the app and services alive. Explicit
  Stop addresses only currently connected machines, once per host, including
  still-responsive connections retained from closed projects. Never reconnect
  saved/disconnected servers to stop them. In-flight attaches prevent stopping
  that host until finished. Stop failures keep the app open and report partial
  success without automatic retries. `stop_daemon` leaves kernels, terminals
  and jobs alive; the dialog states this. Ordinary project-window close still
  returns to Welcome rather than prompting to quit.
- A project window reopens its tabs, and each document picks up the sessions that last ran its cells. Its kernel picker offers that project's sessions (and ones started outside any project), not other projects'.
- The folder and tabs the app remembered before projects come back once, as a project, the first time it is opened.

**Built: project settings (2026-10-25).** Points 3 to 5:
- *`.sandhill/project.toml`* holds `[kernels] r` and `python` (a kernel's name, or `"detect"`), `[run] documents` (`"document"` or `"project"`) and `[env] files`. The daemon writes it (`save_project_settings`), changing only those keys so people's comments and other keys stay, and makes `.sandhill/.gitignore` (ignoring `outputs/`) the first time. Nothing is written until someone saves.
- *What the folder points to* (`get_project_settings`): `renv.lock` means the usual R kernel (Ark, else IRkernel), whose library renv loads as R starts in the project's folder; the environments inside the folder (`.venv`, `venv`, `env`, `.conda` and pixi's `.pixi/envs/*`) are kernels of their own, named `project-venv-py`, `project-pixi-default-r` and so on, which `list_project_kernelspecs` lists first; `pixi.toml` and `uv.lock` without their environment, and the conda environment `environment.yml` names, say what to run when there is no kernel yet ("It has no Python kernel: `uv add --dev ipykernel`."). The kernel a language runs in is the saved one, else the first found that is installed, else the usual one.
- *Who follows it.* Documents, scripts, the Console's R and Python buttons, new notebooks and the agents' tools all start the project's kernel, over the one a notebook names (the kernel bar's "New …" can still pick another). A saved kernel this machine doesn't have stops the start with a message saying where it was set, rather than running another.
- *Asked once.* The first time a project without a `project.toml` opens (and some kernel is installed), the window asks "Which R and Python should <project> use?" with what runs now picked; Save writes the file, Not now writes nothing, and either way the window doesn't ask again (it remembers in the project's personal state, not in the folder). Project settings… in the ☰ menu and the project menu bring it back. At first (#55) it asked only when there was a choice (an environment found, or two kernels of one language), so a server whose one Python with ipykernel is a conda env, and a folder pointing nowhere, never asked. A language with one kernel says that only environments with a kernel (ipykernel, Ark or IRkernel) are listed.
- *Seeing the file.* The file tree shows names that start with a dot (`.sandhill`, `.env`, `.gitignore`) except git's folder and what macOS, Jupyter and RStudio leave beside the files (`.DS_Store`, `.ipynb_checkpoints`, `.Rproj.user`), as VS Code does; at first it hid them all, so `project.toml` and `.env` couldn't be seen or made there.
- *Working directory.* With `documents = "project"`, notebook and Quarto chunks run in the project's folder like the Console and scripts; sessions already running keep theirs until restarted.
- *Startup.* Kernels start with the env files' variables (python-dotenv's format: quotes, `export`, `${NAME}` and `${NAME:-default}`), which don't replace what activates a kernel's environment. R kernels start in the project's folder and then `setwd()` to the document's.

Not yet: the output cache in `.sandhill/outputs/`; a personal override for a collaborator whose machine lacks the saved kernel, short of changing the shared file.

---

## 6. Performance budget (targets, not measurements)

| Component | Target |
|---|---|
| Mac app, idle with 3 docs open | < 150 MB total (the app, plus a WebKit content process when HTML outputs show) |
| Cold start to editable | < 500 ms with outputs coming from the local cache, then sync with the server |
| Reopen a notebook with 200 outputs | Only the visible outputs are mounted; the rest are lazy |
| Server daemon | ~10–30 MB RSS. Kernels, LSPs and agents dominate, and they would exist in any IDE. |
| Per-keystroke work | All on the Mac (CodeMirror). Only LSP requests and executions cross ssh. |

Keep the daemon protocol binary (msgpack), stream outputs incrementally, and never send blobs the client already has (content hashes).

How notebooks stay small in the app (decided October 2026, after measuring three scanpy notebooks of 7, 7 and 25 MB): a `.ipynb` is mostly base64 plots, and every copy of that text costs memory in each process it passes through, which the allocator then keeps. So plots never travel as text. The daemon opens a notebook with `read_notebook`, which moves plots and outputs over 64 KB into the blob store and sends references; the web view loads them by URL. Saving sends the references back with `write_notebook`, and the daemon writes the values into the file. In the web view, images load lazily, far-away images are dropped until they scroll near, and off-screen outputs skip layout (`content-visibility`). After a large message the daemon and the app hand freed memory back to the system.

Notebooks of 100 MB and more (October 2026, measured in Chromium on a 106 MB notebook of 528 cells with 440 plots and 220 tables): a notebook's outputs are drawn only once their cell comes within two screens of view, so opening it with every output shown, or pressing Show outputs, draws only a few (Show outputs went from a 2.9 s freeze to 0.3 s). Each cell's code and outputs skip layout and paint far from view (`content-visibility` on `.nb-body`), taking a height the app guesses from the cell until first drawn, then their last height; scrolling the whole notebook went from 223 to 50 slow frames (over 50 ms) of 2220, and from 10.4 s to 2.6 s of blocked time. Containment sits on the body, not the cell, so the insert buttons that hang below a cell aren't cut off, and a cell being edited drops it so the editor's pop-ups aren't either; a `:has(.cm-editor)` selector did this at first and cost more than everything it saved. A plot keeps the shape it loaded with when its pixels are let go, without reading layout. Tables over 200 rows show the first 200 and a button for the rest. `content-visibility` needs Safari 18; older versions ignore it and still get the rest.

In the GPUI app (2026-11-05, measured on Huan's 190 MB notebook of 136 cells and 68 plots, 272 megapixels): GPUI lays out, prepaints and paints the whole element tree each frame, and a wheel turn redraws the window, so with every cell in the tree each frame cost the same wherever the view was, 28 ms in a release build and 221 ms in `cargo run`'s unoptimised one. The plots were not the cost (0.5% of the frame); the 136 editors (28%) and the cells' frames and outputs were. Now only the cells within a screen of the view are drawn (`cell_view::Placed`, `Workbench::place_cells`); each other cell is space of its height at the last layout, read from the scroll handle's child bounds, so the children, and scrolling to a cell, stay as they were. The cell being edited, run or lit up is always drawn, and a web page under a cell not drawn is put away like one scrolled out of view. A cell not laid out yet is drawn, which measures it, so opening a notebook still draws it whole once. A cell that changes height far from view (a run's last output) jumps into place when it comes near. Scrolling the whole notebook went to the display's 16.7 ms a frame in both builds, with about 9 ms of work in the unoptimised one; `app/Cargo.toml` also builds the dependencies optimised in dev builds.

Saved outputs start hidden (decided October 2026, Huan's call). Each shows as its status line with a summary ("plot", "2 plots, table") and opens on a click; Show outputs and Hide outputs act on the whole document, and running a cell shows its output. A hidden output is not rendered at all, so its plots are never fetched or decoded. Which outputs are shown is kept per document in the project state, not in the file. A notebook cell shows syntax-highlighted code until it is clicked or focused, and only then gets a CodeMirror editor; far from view it gives the editor up again, keeping its state (text, selection, undo history) for the next one. Making an editor for every cell that scrolled by cost the web view about 30 MB in a 91-cell notebook, mostly CodeMirror code compiled by WebKit's JIT; the measurement that showed it was turning the JIT off (`JSC_useJIT=false`), which removed the growth.

---

## 7. Build order (suggested MVP path)

1. **Daemon core:** systemd user service, ssh stdio attach, kernel manager for ipykernel and Ark, IOPub journal, reconnect and replay. Test it with a CLI client before any UI exists.
2. **Mac shell:** Tauri + CodeMirror, open a `.qmd` over ssh, run a chunk, show inline output, sidecar cache, close and reopen to see the outputs again. *This is the "essential" loop; get it right first.*
3. **PTY host + Claude Code / Codex** in a terminal pane on the server, with detach and reattach.
4. **MCP server** for kernels and notebooks, so agents can run chunks and read outputs.
5. **`.ipynb` editor** on the same cell, kernel and output components as Quarto, plus **Share** (export a `.qmd` to a single `.ipynb` or self-contained HTML from cached outputs, without re-running).
6. Variables, data explorer and plot panes (reuse Ark's comm protocol for R).
7. Native chat panel over ACP (Codex with a ChatGPT login or API key; Claude with an API key), plus an agent settings page: subscription or API key per agent, Anthropic or OpenAI format, base URL.
8. Jobs view (`systemd-run` scopes, logs, resource usage).

Reusable open-source parts: **Ark** (R kernel and LSP, MIT), **runtimelib / jupyter-protocol** (Rust Jupyter client from nteract, which the daemon builds on), **`jupyter_client`** (the official Python client, as the reference for protocol behavior), **Air** (R formatter), **ruff / ty**, **CodeMirror 6**, **xterm.js**, **Quarto's** own `quarto inspect` for chunk parsing, and Positron's comm protocol definitions as a reference (Positron itself is Elastic License 2.0, so reuse its ideas rather than its code).

---

## 8. Market survey: tools like what you want (besides Positron)

| Tool | Tech | R + Python | Quarto / marimo inline, saved | Remote + persistent | AI with a subscription | Gap vs. your wish list |
|---|---|---|---|---|---|---|
| **[Zed](https://zed.dev)** | Rust, GPUI, native; no Electron | REPL with ipykernel and **Ark** for R ([docs](https://zed.dev/docs/repl.html)) | Inline REPL output in plain files with `# %%` cells. Persistence across sessions is not documented. No Quarto or marimo notebook UI. | SSH remote dev; remote kernel support added recently ([commit](https://git.secluded.site/zed/commit/ae9bb6a6287db505850eb4ff9f30c9a7eeae40b2)) | Claude Agent and Codex over ACP ([docs](https://zed.dev/docs/ai/external-agents)) | **Closest non-Electron option today.** Missing saved outputs, Quarto/marimo, and a daemon that survives the app. |
| **[nteract 2.0](https://www.nteract.io/blog/nteract-2.0)** | Tauri + Rust daemon + Automerge (rebuilt from Electron, March 2026) | Jupyter kernels; R not highlighted | `.ipynb` only | Local only for now; remote runtimes are planned | Agents (Claude Code, Codex…) join notebooks through MCP and the `runt` CLI | **Closest architecture to my recommendation.** Missing remote, Quarto and marimo. Worth reading its source. |
| **[marimo](https://marimo.io)** (run on the server, use in a browser or the VS Code extension) | Python + web | Python only | marimo, saved via the session cache | Server keeps running if you start it detached | ACP agent panel (Claude Code, Codex) and `marimo pair` ([docs](https://docs.marimo.io/guides/editor_features/agents/)) | No R, not an IDE, and you need something else to keep it running |
| **RStudio Server / Posit Workbench** | C++ backend, browser UI (RStudio Desktop has used Electron since 2022) | Strong R, OK Python | Rmd/qmd inline output, cached and restored | Server sessions persist when the browser closes (idle suspend is configurable) | Limited | Browser-based, R-centric, weak agent story |
| **JupyterLab + [Jupyter AI v3](https://jupyter-ai.readthedocs.io/en/stable/releases/v3.1.0.html)** | Python server, browser | Any kernel | `.ipynb` (jupytext for qmd-ish files) | Kernels survive on the server | ACP agent client (`jupyter-ai-acp-client`) | Browser, no Quarto inline, outputs during disconnects are not reliably kept |
| **[Erdos](https://github.com/lotas-ai/erdos)** (Lotas) | VS Code fork (Electron), AGPL | R, Python, Julia consoles | Inline qmd/Rmd | SSH and containers | Own AI backend or BYO key | Electron, essentially "another Positron" |
| **VS Code / Cursor / Windsurf** + Remote-SSH + Jupyter/Quarto + Claude Code or Codex extensions | Electron | Yes, through extensions | `.ipynb` yes; Quarto inline is weak | Remote-SSH, but no persistent session | Claude Code and Codex extensions use your own login | Electron and RAM. The "works today" baseline. |
| **PyCharm** (DataSpell is being sunset in 2026 ([news](https://www.neowin.net/news/end-of-an-era-jetbrains-is-killing-this-popular-ide-used-by-python-devs/))) | JVM | Python strong, R through a plugin | `.ipynb` | Remote interpreters and Jupyter | Codex with ChatGPT login, plus Claude through AI Assistant ([JetBrains](https://www.jetbrains.com.cn/en-us/help/ai-assistant/codex-agent.html)) | Heavy RAM, no Quarto/marimo |
| **Emacs** (ESS + org-babel or quarto-mode) / **Neovim** (quarto-nvim + molten.nvim) + claude-code / codex in a terminal | Native terminal apps | Yes | org-babel stores results *inside* the file; molten can save outputs to JSON | Runs over ssh; an Emacs daemon survives disconnects | The official CLIs in a terminal | The power-user route, which meets most of the list today if you accept a terminal UI |

**Takeaway from the survey:** no shipping product combines *non-Electron, Mac-native, R and Python, Quarto and marimo inline outputs that persist, a remote daemon that survives closing the app, and subscription-based agents*. The nearest pieces are Zed (native editor, agents, remote) and nteract 2.0 (Tauri, Rust daemon, outputs as blobs, agents via MCP). Your design is roughly "nteract's daemon idea, moved to the server, plus Quarto, plus the official CLIs as agents".

---

## 9. Risks and open questions

- **Quarto editing UX** (embedded R and Python in one file, LSP for each chunk) is the hardest editor work. Quarto's VS Code extension solves it with per-chunk virtual documents; copy that approach.
- **Anthropic's policy can change.** Isolate the agent layer so that switching the Claude path between "CLI with subscription" and "API key" is a setting.
- **WKWebView quirks** with some htmlwidgets: test early with plotly, DT, leaflet and Seurat plots.
- **Multi-user servers:** if colleagues share the Ubuntu box, each user runs their own daemon, so no root is needed. Keep it that way.
