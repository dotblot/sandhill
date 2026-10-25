# A Mac-native AI coding IDE for R/Python bioinformatics: recommended framework

*Research note, 2026-10-10, updated 2026-10-11: scope narrowed to Jupyter (`.ipynb`) and Quarto (`.qmd`/`.Rmd`); marimo dropped.*

*Original note, 2026-10-10. Nothing is built yet; this is a design recommendation. Claims about existing tools were checked on the web on this date (links inline). Where something is my inference rather than documented, it says so.*

---

## 1. The recommendation in one screen

| Layer | Pick | Why |
|---|---|---|
| Mac app shell | **Tauri 2** (Rust core + the system WKWebView) | No Electron or bundled Chromium. Idle RAM is typically tens of MB, not hundreds. You need a web view anyway because notebook outputs (plotly, htmlwidgets, interactive tables) are HTML. |
| UI | **SolidJS** (chosen over Svelte), **CodeMirror 6** editor, **xterm.js** terminal | Small, fast, and CodeMirror handles very large files well. JupyterLab's editor is also CodeMirror 6, so its Python/R language modes are reusable. |
| Mac ↔ server link | **System `ssh` (your `~/.ssh/config`, keys, ProxyJump)** carrying one multiplexed RPC stream to the daemon's stdio (msgpack or JSON-RPC) | This is the pattern VS Code Remote and Zed remote use. It needs no open ports and works through bastions. The app auto-uploads the daemon binary on first connect. |
| Server daemon | **One Rust binary (tokio)** running as a **`systemd --user` service with `loginctl enable-linger`** | It survives the Mac app closing, network drops, and your logout, with no tmux. It owns kernels, jobs, PTYs, the output store and the agents. |
| Kernels | **Jupyter protocol over ZeroMQ**: `ipykernel` for Python, **Ark** (Posit's Rust R kernel, MIT) for R | These are standard and reconnectable. Ark already speaks the variables, plots and help "comms" that Positron uses. Use the `runtimelib` / `jupyter-protocol` crates from nteract's runtimed. |
| Notebooks | **Jupyter `.ipynb`** (outputs stored in the file) and **Quarto `.qmd`** (and `.Rmd`) with a sidecar output store, plus one-click export of either to a single shareable `.ipynb` | See §4. Both run on the same kernels, and outputs are saved and come back on reopen. Your existing notebooks open as they are. |
| AI agents | Run the **official, unmodified `claude` and `codex` CLIs on the server**, under the daemon, each logged in with your own subscription **or an API key** (Anthropic or OpenAI format, with a custom base URL for compatible providers). Give both an **MCP server** exposed by the daemon ("run chunk", "read outputs", "list variables", "tail job log"). | This is the only clean way to use subscriptions without an API key. Running them server-side means agent work also survives the Mac closing. See §5. |
| Language servers | Run on the server through the daemon: **basedpyright** or **ty**, plus **ruff** (Python); **Ark's built-in LSP** and **Air** (R formatter) | Analysis runs where the code and packages are. |

**In short:** Tauri on the Mac (TypeScript UI, a thin Rust layer), a Rust daemon under systemd on Ubuntu, Jupyter kernels (ipykernel and Ark) behind both `.ipynb` and Quarto, sidecar output caches for Quarto, and Claude Code and Codex running unmodified on the server, with an MCP bridge into your live kernels.

---

## 2. Architecture

```
┌──────────────── macOS (Apple silicon) ────────────────┐
│ Tauri 2 app (Rust core, ~1 process + WKWebView)       │
│  ├─ Editor: CodeMirror 6 (ipynb / qmd / Rmd / py / R) │
│  ├─ Inline outputs: sandboxed <iframe> per output     │
│  ├─ Agent pane: xterm.js attached to a server PTY,    │
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
Rust for both the daemon and the Tauri core, chosen for speed and low RAM. You are new to Rust, so AI agents write most of it and the Rust compiler acts as a strict reviewer. The UI is TypeScript. Keep the Rust code in small, well-tested crates (protocol, kernel manager, output journal, ssh link, agent host) so each piece is easy for an agent to work on alone.

### Local mode on the Mac (added 2026-10-11)
The same daemon also runs on the Mac itself. "Local" is just another host:
- Build `sandhilld` for `aarch64-apple-darwin` as well as Linux. On the Mac it runs as a **launchd user agent** instead of a systemd service, and the app talks to it over a Unix socket instead of ssh. Everything above the link (kernels, output journal, agents, MCP tools, sidecar outputs) is shared code.
  - As built (step 2): the app ships `sandhilld` inside `Sandhill.app` and runs `sandhilld attach --start`, which starts the daemon in the background if it isn't running, so no launchd setup is needed. If the running daemon is another build (an older app left it running), `attach --start` stops it and starts its own; kernels survive that, as they survive any daemon restart. The remote case runs the same command through ssh (next section). The launchd agent in `packaging/` stays optional, for starting the daemon at login.
- Keep the OS-specific parts behind one small trait from day one: service install (systemd vs launchd), job isolation (`systemd-run` scopes vs process groups), and the transport (ssh stdio vs Unix socket).
- Closing the app window doesn't stop local work, because the daemon keeps running. But work does stop while the Mac **sleeps or shuts down**. The daemon should hold a sleep assertion while jobs run (what `caffeinate` does), and warn before you close the lid with work in progress.
- The kernels (ipykernel, Ark) and agents (`claude`, `codex`) run natively on Apple silicon.
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
- **The start screen** lists servers next to "This Mac"; "Connect to Server…" adds one, and the list is kept in the app's `servers.json`. A window showing a project on a server is titled "project — server".

Not yet: password prompts, forwarding ports for the Viewer pane, Mac servers (the app would need to ship its own Mac build as one), and a server that can't reach `~/.local/bin` (a read-only home).

### Why a custom daemon and not Jupyter Server?
Jupyter Server keeps kernels alive after the browser closes. But in the classic setup, outputs a kernel emits while no client is attached go nowhere: IOPub is publish/subscribe, so a disconnected client misses them. (That is my inference from the protocol; JupyterLab's newer server-side document work narrows this gap.) Your core need is "close the laptop, come back, see everything that happened". So the daemon itself must subscribe to IOPub and journal every message to disk. On reconnect the Mac asks for "everything since sequence N" and replays it. This is the same idea nteract 2.0's `runtimed` uses locally: a per-user daemon that owns kernels, with outputs kept as content-addressed blobs ([architecture](https://mintlify.wiki/nteract/desktop/development/architecture-deep-dive)). You are adding "remote" and "journal while nobody is watching".

### Surviving restarts
- **Mac app closes or the network drops:** nothing on the server notices. The daemon keeps journaling.
- **You log out of the server:** `loginctl enable-linger $USER` keeps user services running.
- **The daemon itself restarts (an upgrade):** start kernels and jobs in their own systemd scopes (`systemd-run --user --scope`) so they are not its children. Jupyter kernels are network servers described by a connection file, so a restarted daemon can reconnect to a running kernel. Outputs emitted during the few seconds the daemon is down are lost. Batch jobs write to log files, so they lose nothing.
- **Long analyses** (alignment, Seurat/Scanpy pipelines): encourage "run as job" (script → log + result files) rather than a 10-hour notebook cell. The UI can still render the job's plots and tables inline.

---

## 3. Mac app choice: why Tauri over the alternatives

| Option | Verdict |
|---|---|
| **Tauri 2** (recommended) | Rust backend, system WKWebView, small binaries. One codebase for the editor, notebook cells and outputs. Downside: WKWebView is Safari's engine, so test plotly and htmlwidgets there. That is fine in practice. |
| Native SwiftUI/AppKit + WKWebView only for outputs | The most "Mac", with the lowest idle RAM. But you would build a code editor from scratch (or STTextView/Runestone) with LSP, multi-cursor and folding. That costs months before any data-science feature exists. A good choice only if UI polish matters more than time. |
| Fork **Zed** (Rust/GPUI) | The fastest editor available, with ACP agents, SSH remote and a REPL already built. But GPUI has no HTML engine, so htmlwidgets, plotly and interactive tables are hard. Its upstream also moves very fast. Worth studying, not forking. |
| Electron / VS Code fork | Excluded by you. It is also what Positron, Cursor and Erdos are. |

### The look (added 2026-10-11)

The app follows the UI mockup, the agreed final layout, kept in
`docs/design/ui/`: `spec.md` describes it, `sandhill-layout.html` is the
clickable mockup (light and dark) and the PNGs are screenshots of it. Where
the spec and the mockup differ, the mockup wins. The app follows draft 17
(2026-10-12) and, since 2026-10-25, the title bar and machine tabs of drafts
20 to 35; the top of `spec.md` lists what each draft changed.

**Title bar and one machine at a time (built 2026-10-25).** A 31px title bar
row holds three capsules, fully rounded bars: after the window buttons, the
toolbar (left pane, Agents, right pane | ⌘K, menu); centred in the window, a
tab per machine with its project's path, an orange dot when an agent there
waits for you, and + to connect to another; at the right, the edit capsule
(undo, redo | cut, copy, paste | save, save all ⌥⌘S), whose buttons act on
the editor last in focus and dim when they can't. On macOS the windows use an
overlay title bar with the window buttons centred on the capsules
(`title_bar()` in `app/src-tauri/src/windows.rs`). Everything under the title
bar belongs to the machine picked there, so local and remote work never mix:
`app/src/ui/Workbench.tsx` keeps one project view per machine tab (and per
project switched to with the project name at the top of the left pane) alive
while hidden, so switching back returns to the same tabs, Console session,
plot and conversation. The Start screen opens in the window, under the same
title bar, and a project opened there goes into its machine's tab.
Right-clicking a tab closes it. The app remembers each window's tabs for the
next launch (`open-projects.json`); a list from before machine tabs opens as
one window. Since a window can use several machines, the app sends each
daemon message with its host's name (`{ host, body }`).

**Pane tabs (built 2026-10-25).** Every pane's tabs sit in a capsule like
the title bar's, the picked tab a raised pill, and the tab bars (and the
editor's kernel bar) are liquid glass floating over the top of their pane:
the content starts under them and scrolls up beneath them, blurred. Only the
bars blur; the panes still don't. The editor keeps the cursor clear of the
bars (`underBars` in `app/src/editor/editing.ts`). Only the title bar drags
the window. No text is smaller than 10px, the smallest size macOS uses.

**The file tree (built 2026-10-25)** follows drafts 26–28: a chevron column,
an icon column, 16px per level and plain guide lines. New files and folders
are named in a field in the tree, from the + menu (in the selected folder),
a folder's hover buttons, the right-click menu, ⌘N and ⌥⌘N; a name such as
`raw/samples.csv` makes the folder too, and new Quarto documents, R scripts
and notebooks start from a template (`app/src/model/files.ts`). The right-click
menu renames, duplicates, copies the path and moves to the Trash, plus
Reveal in Finder on this Mac, and Download to this Mac and Delete… on a
server. These go through the daemon (`make_dir`, `move_path`, `copy_path`,
`delete_path`, `read_bytes`; see `docs/protocol.md`), so they work the same on
a server. The daemon never replaces anything: a clash comes back as an
`exists:` error, which the field shows in red. Renaming or deleting closes
the documents open from it first, and refuses while one has unsaved changes.
The tree sorts by name, size (folders by the sum of their files) or date
modified, folders first; the order is remembered on the Mac.

**Import and column stats (built 2026-10-25).** Variables has an Import menu,
as in RStudio: for R, text (base and readr), Excel, SPSS, SAS, Stata, R data
and 10x Genomics; for Python, pandas, Parquet, AnnData and 10x. Each opens a
dialog that browses the daemon's machine for files it reads, names the
variable after the file and shows the code before running it in the session,
so the run shows in the Console (`app/src/model/imports.ts`). The Data
Explorer's column list and stats hide with **Hide column stats**, remembered
on the Mac.

Panes sit 3px below the title bar, 3px apart and 3px from the window's edges;
the gaps are drag handles. The editor and Console are translucent cards on a
warm gradient, each with minimize and maximize buttons; the left pane sits
directly on the background, with the project's name (a button that lists the
machine's projects), its files, its kernel rows (language, version, files
using it, memory, and shutting down with an inline confirmation) and the
machine's CPU, memory, GPU and disk (`system_stats`).
One corner radius, 12px, for every rounded thing. All text is 13px, the macOS
standard size, except the kernel rows, resource rows, Git pane and pop-ups at
11px, and nothing is below 10px. Chrome is in the system font. Code, outputs,
Variables, the console and the agent terminals are in Maple Mono NF, the Nerd
Font build of Maple Mono, bundled with the app (OFL) so every Mac shows the
same font and shell prompts can show their icons; the Data Explorer uses the
system font. Code ligatures are on, so `<-` shows as an arrow while the file
keeps what was typed. Plots stay white in dark mode. The panes skip
`backdrop-filter`: over a smooth gradient a blur changes nothing you can see,
and each blurred pane costs a window-sized compositing layer. Styles are in
`app/src/styles.css`, icons in `app/src/ui/icons.tsx`.

Kernel pickers, in the editor and the Console, show the language icon and
version; their drop-down holds the kernel, environment, machine, the files
using the session, its memory and the session switch, so session IDs appear
only there. The Agents pane came with step 4 (section 5); its effort control
is a track with a brain for a knob that turns into a star field at the top
level, and plan usage shows as rings. The right pane (⌥⌘B) came with step 5
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
- **The CLIs stay unmodified.** Sandhill configures Claude Code and Codex only through command-line flags (`--mcp-config`, `--settings`, `--append-system-prompt`, `--model`, `--effort`, `--permission-mode`; Codex's `-c` overrides). It never edits their config files and never reads their credentials; each signs in with its own login flow (Claude Code in its terminal; Codex through app-server's login, which Codex completes and keeps). Flags an older CLI lacks are left out (the daemon checks `--help`).
- **Agent state** comes from Claude Code's hooks (prompt, tool, stop, notification) and its status line, which run `sandhilld terminal-event`, and for Codex from what app-server tells Sandhill's bridge. That gives the busy and waiting dots, the "waiting for you" notice, the model, the permission mode (so a mode switched with ⇧Tab shows on the button), the context bar and the plan's 5-hour and weekly use. A status line the user set up themselves keeps working: Sandhill's status line runs it and prints its output.
- **MCP tools, first set:** `list_sessions`, `list_variables` (a quiet, history-free execute with summaries of Seurat, SingleCellExperiment, AnnData and data frames), `run_code` (in the user's session, where results stay) and `get_outputs` (a document's saved outputs, with plots as images). The server is `sandhilld mcp`, which talks to the daemon over its socket. Not yet: `inspect`, jobs, and variable snapshots for busy kernels (listing a busy session says so instead).
- **Code goes into the document (2026-10-12).** Asked to compute or plot something, the agent writes the code into the document with `add_cell` rather than running it out of sight, as Posit Assistant does: below the user's current cell (else after the last cell) in the document the conversation is about (else the one in front), and it runs there, in the document's own session. The output, errors and plots go back to the agent. A failed cell is fixed in place with `edit_cell`. `run_code` stays for quick looks.
  - *Where the user is.* Each window tells the daemon its document in front, current cell, line and selection, and which documents it has open (`set_focus`). The daemon keeps the latest place per project, also with the Mac closed. Agents read it with `get_focus`, and Claude Code's prompt hook adds it to every prompt, so "fix this cell" works.
  - *Who edits.* When a window has the document open, the daemon asks that window to make the edit (`add_cell` request, answered with `answer`): it brings the tab forward, inserts the cell as typing would (one ⌘Z undoes it), moves the cursor into it, saves, and runs it as the Run button does. Editing under an open editor would instead race the user's unsaved changes. When no window has it open, the daemon edits the file and the tool runs the cell in the session that last ran the document's cells, else one in the document's folder with the kernel the document asks for. The chunk ids are the app's (label, else `chunk-<n>`); the daemon finds chunks with the same rules (`crates/daemon/src/qmd.rs`, kept in step with `app/src/model/qmd.ts` by `tests/fixtures/chunks.json`).
  - *Order.* The project's place moves to each new cell, so a series of cells reads top to bottom.
- **The app** has the Agents pane, a Terminal tab in the Console, and reloads open documents when their file changes on disk (checked every 1.5 s; unsaved changes are never replaced without asking).
- **The chat pane (draft 18, 2026-10-12).** Claude Code shows as a chat, after Posit Assistant, while the unmodified `claude` still runs in its terminal underneath; nothing goes through the Agent SDK or Claude Code's credentials, so it stays on the allowed side of the policy above.
  - *Reading.* Claude Code's hooks record the conversation (`chat.jsonl` next to the terminal's output): the user's messages with where they were, each tool call and its result, the reply text, and what Claude asks. The app draws it as messages, step rows that open into code, diffs and outputs (with plots), and a footer per turn. Like the terminal output, it is kept by the daemon and replayed when the app comes back.
  - *Answering.* Claude Code's `PermissionRequest` hook waits for the app's answer (`answer_terminal`), so approvals, questions (`AskUserQuestion`) and plans are cards in the chat: Allow once, for this chat, or for this project (Claude Code's own rule suggestions); question options; carry out a plan or keep planning.
  - *Writing.* The message box types into the terminal (a bracketed paste, then Enter). The first message is passed on the command line, so a new conversation starts with it. The model and effort go through `/model` and `/effort`, the mode through ⇧Tab (read back from the screen), and stopping is Esc. The terminal is one click away (gear menu), for logins, slash commands and anything the chat doesn't show.
  - *Not yet:* switching a running Claude conversation to YOLO, the Restricted and Ask modes, renaming and exporting conversations, and resuming an ended one.
- **Codex in the chat (2026-10-17).** Codex runs as `codex app-server`, its documented JSON-RPC interface for other apps, under a small bridge in the terminal (`sandhilld codex-chat`). The bridge speaks app-server's protocol and writes the same conversation entries and reports Claude Code's hooks write, so the app shows both agents with one chat. The terminal shows a transcript.
  - *Steps.* Commands, file edits (`apply_patch`, shown as diffs), Sandhill's MCP tools, web searches, Codex's thinking summaries and its plan (as a to-do list) are step rows.
  - *Approvals.* Codex's requests (a command, an edit, an MCP tool call, more access, a question) are cards; each names its step. The buttons are the ones Codex offers: once, for this chat (`acceptForSession`), or for this project (a command prefix rule Codex saves).
  - *Writing.* The app sends what the user does as commands (`tell_terminal`): a message (queued while a turn runs), stop, compact, and the model, effort and mode, which apply from the next turn. Modes map onto app-server's settings per turn: Normal asks before leaving a read-only sandbox, Auto writes in the project and lets Codex's reviewer decide, YOLO has no sandbox and never asks, Plan uses Codex's plan mode and then asks to carry the plan out (yes switches to Normal). The model menu lists the models Codex offers, with each one's effort levels.
  - *Signing in.* When Codex needs a ChatGPT login and has none, the chat offers app-server's sign-in: in the browser (on this Mac), or with a code on any device (on a server).
- **Next:** omp over ACP as a chat view for open models, and the context breakdown.

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
| **Viewer pane** (htmlwidgets, HTML reports, Shiny) | v1 | A sandboxed WKWebView pane. Ports are forwarded over ssh. |
| **Quarto inline output** | MVP | Already the core of §4. |
| **Interpreter discovery and switching** (conda/mamba, venv, uv, renv, several R versions) | v1 | The daemon scans the server and remembers a choice per project. Started: kernels inside conda environments, pyenv versions and the shell's environment are found without `ipykernel install` and run with their environment activated, including environments that have ipykernel but no `kernel.json` (Jupyter starts those too). |
| **MCP server for coding agents in live sessions** (Positron 2026.10, experimental) | MVP | Already the core of §5. Positron adding the same thing confirms the design. |
| **Assistant aware of your session** (variables, plots, errors) | v1 | The same MCP tools, plus "Fix" and "Explain" buttons on errors. |
| **Packages pane** | v1 | Built 2026-10-18: see below. |
| **Run App** (Shiny, Streamlit, Dash…) | later | A job plus the Viewer pane. |
| **Data Connections pane** (databases, warehouses) | skip | Rarely needed for bioinformatics. |
| VS Code extension marketplace | **skip** | This is where most of Positron's memory goes. Build in what you need instead. |

**Built: the shared Console and Variables (step 5, part 1, 2026-10-18).**
- *One Console per session, not per file.* As in RStudio and Positron, the Console shows a session, whatever ran the code: its prompt, a script's ⌘↵ or an agent (marked with its name). Pills in the Console's tab strip switch sessions; the session in front follows the document in front and the script whose line just ran.
- *Notebooks keep their output to themselves (2026-10-24).* By default a notebook or Quarto cell's run stays under the cell: the Console doesn't show it, and doesn't switch to or list a session only notebooks have used (Variables, Help and Packages still follow the notebook's session). The Console's R and Python buttons then use the session the notebooks run in rather than starting another. The ☰ menu's "Notebooks and Quarto" choices bring their runs into the Console (echoed in two lines, with the output left under the cell) and their plots into Plots. Both are kept per Mac. Every run is already journaled, so the Console is the session's journal (`session_runs`), not something the app keeps; Clear hides what came before on this Mac only.
- *The prompt* is a small CodeMirror editor: Enter runs the code once it is complete (brackets and strings closed, no trailing operator in R, no open block in Python), otherwise it starts a new line; ⇧Enter always does; ↑/↓ walk the session's history, Ctrl-C interrupts and Ctrl-L clears.
- *Variables* uses neither Ark's comm nor the debug protocol for now: the daemon runs a quiet, history-free snippet in the session (`list_variables`), which works the same for Ark, IRkernel and ipykernel. It knows data frames, factors, lists and dicts, Seurat and SummarizedExperiment objects, and AnnData (`X`, `obs`, `var`, `obsm`, `layers`, `uns`, `raw`) with the columns of `obs` and `var` a click away. The list is read again after each run in the session, never while it is busy. Ticked variables can be deleted (`delete_variables`), which also runs `gc()` / `gc.collect()` and, in Python, drops IPython's references to old outputs, so the memory really comes back; the pane says how much. The agents' `list_variables` tool reads the same list.
- *Agent code in the Console.* Under the modes, "Agent code goes: In the file / In the Console" decides what the agent's `add_cell` does: add a cell to the linked file (the default), or run the code in the document's session, where it shows in the Console as the agent's. The choice can change while the agent runs.

**Built: Plots (step 5, part 2, 2026-10-18).**
- *One history for the project.* The right pane is now two cards, Variables over Plots, with a gap between them to drag; each minimizes and maximizes on its own. Plots shows every image the project's live sessions drew, R and Python together and in time order, whatever ran the code: the Console, a script or an agent, and a notebook or Quarto chunk (labelled "analysis.qmd · umap" when the chunk has a label) when the ☰ menu says so. A new plot comes to the front as it is drawn; the arrows, ← and →, and the thumbnails go back through the rest.
- *From the journal.* Outputs are already journaled, so the pane reads each session's latest 100 images (`session_plots`) when it opens and then follows the sessions' events; nothing is kept by the app. Clear hides the plots so far on this Mac only; they stay in the Console and under their cells. A session's plots go when the session ends, as in RStudio.
- *Zoom, copy and save.* The magnifier opens a plot large. An SVG plot can be saved as SVG, or as a PNG drawn from the SVG at a size you type or pick (4 × 3 in, 6 × 4 in or a 16:9 slide, at 300 dpi, written into the PNG so Word and PowerPoint place it at that size); the plot keeps its shape on a white background. A PNG plot is saved or copied as the kernel drew it. Save asks for a path in the project, `figures/<chunk label or file name>.png` at first and then the folder used last, and asks before replacing a file; the daemon writes the bytes (`write_file` with `base64`). Copy puts a PNG on the clipboard.
- *Not yet: re-drawing.* Positron re-draws a plot at the pane's size and at the size you save it, through Ark's plot comm (R) and by keeping matplotlib figures (Python). Here plots show as the kernel drew them, so a PNG doesn't get sharper when the pane grows. That needs the plot comm, and comes later; ask a kernel for SVG (`%config InlineBackend.figure_formats = ["svg"]`, or `options(jupyter.plot_mimetypes = "image/svg+xml")` in IRkernel) to save at any size today.

**Built: Data Explorer (step 5, part 3, 2026-10-18).**
- *A table as an editor tab.* The table button on a data frame or matrix in Variables (`seu@meta.data`, `adata.obs`, a plain matrix or NumPy array) opens it as a tab beside the documents, with a table icon and the table's code as its name. The bar over it says the session, the table and whether it is live; Copy as code and Export CSV sit on the right. The status bar says how many rows show ("618 of 2,638 rows"). Tabs aren't kept between launches.
- *Computed in the kernel, like Positron.* Neither Ark's data-explorer comm nor a comm of our own for now: the daemon runs a quiet snippet in the session for each request (`table_rows`, `table_summary`, `table_export`), the way Variables does, so it works the same in Ark, IRkernel and ipykernel. Filtering and sorting happen in R or pandas, and only a page of 200 rows comes back. The grid draws only the rows and columns on screen and reads more pages as you scroll; a 1,000 × 1,000 array or a table of a million rows scrolls the same as a small one. The first 500 columns of a wider table show.
- *Columns at the side.* Each column has a 20-bin histogram (numbers) or bars of its commonest values, colored for a factor or category of up to 10 values (the same colors in its cells). The picked column's min, median, mean and max, or its values with counts, show under the list; clicking a value filters to it.
- *Filters and sorting.* Add filter asks for a column, a condition that fits its kind (`<`, between, is one of with the values to tick, contains, is missing…) and the values; chips over the grid change or remove them. Clicking a column's name sorts by it, ascending, descending, then not; shift-click sorts by another column next. Missing values sort last and pass no filter but "is missing".
- *Live.* After each run in the session the table is read again, keeping the filters and sorting on their columns by name, so a new column or a changed value shows at once. While the session runs code the last rows stay and a note says so.
- *Copy as code and Export CSV.* Copy as code puts R or pandas code that does what the view does on the clipboard (`x <- meta; x <- x[which(x$percent.mt < 5), , drop = FALSE]; x <- x[order(-xtfrm(x$nCount_RNA)), , drop = FALSE]`). Export CSV writes the rows shown, filtered and sorted, to a file in the project (`write.csv`, `to_csv`).
- *Not yet.* Opening a table from code (`View(df)` in R, a `%view` magic in Python); polars and Arrow tables; editing cells; and keeping the filtered, sorted order between requests, which each request recomputes (fine at a few million rows, slow beyond). Ark's data-explorer comm would bring `View()` for R.

**Built: Help and Packages (step 5, part 4, 2026-10-18).**
- *Tabs on the top card.* Variables, Help and Packages share the right pane's top card as tabs, over Plots. All three follow the session in front, the Console's.
- *Help.* Two boxes, as in the mockup: a function or topic, and a package (empty for all). Matches show as you type, six at first: in R every installed package's topics (titles are searched too, so "principal components" finds `prcomp`), in Python the public functions, classes and modules of the modules already imported. Attached or imported packages come first. A click shows the page under the matches, with Back and Forward. R's page is the package's own HTML from `tools::Rd2HTML`, cleaned of scripts; its links open here, "Run examples" runs `example()` in the Console, and web links open in the browser. Python's page is the signature (one argument to a line when long), the docstring read as sections (numpydoc and Google style, doctests and reStructuredText notes), and a module's or class's public names, each a click away; `:func:` references and See Also names are links. Source shows the function's code: R's deparsed, Python's from its file.
- *Help from the Console.* `?lm`, `?stats::filter`, `??pca`, `help(lm)` and `help(package = "Seurat")` in R, and `?np.mean`, `np.mean?` and `help(np.mean)` in Python, open the Help pane instead of running, and go into the prompt's history. F1 on a name in an editor or the Console (or on a selection) opens its help too.
- *Computed in the kernel, again.* Not Ark's help comm or R's help server: the daemon runs a quiet snippet (`help_topics`, `help_page`, `help_source`), which works the same in Ark, IRkernel and ipykernel and over ssh, with no port to forward. R's index of 70,000 topics is read once per session (under a second) and kept until a library changes; a search then takes about 50 ms. Python's index is kept until more modules are imported.
- *Packages.* What's installed for the session's language: R's libraries, or the Python environment's distributions, attached or imported ones first, then the rest with "loaded" where they are. The filter searches names, then titles. A click opens the package's help: its topics and its overview page in R, its module in Python. Install (the + button) runs `BiocManager::install()` in the Console when BiocManager is there (CRAN and Bioconductor), otherwise `install.packages()`, setting the CRAN mirror if none is set; in Python `%pip install`, or `%conda install` with conda-forge and bioconda in a conda environment. Remove asks first, then runs `remove.packages()` or `%pip uninstall` (`%conda remove` for what conda installed). The list is read again after each run, so `library(DESeq2)` shows it attached.
- *Not yet.* Updates (the mockup's "update 1.2.3", which needs CRAN and Bioconductor reached from the session); help for objects that aren't imported in Python (it searches what's imported, and opens any module by name); R's vignettes; and math in R pages, which shows as LaTeX.

**Built: Git (step 5, part 5, 2026-10-18).**
- *A tab on the top card.* Git joins Variables, Help and Packages as a tab, and the card grows to two thirds of the pane while it shows. It follows the project's repository, not a session. Everything in it is 11px and never bold, as the spec says.
- *The header.* The branch (a click opens the Branches menu), how far it is ahead of and behind its remote branch, and buttons for Fetch, Pull, Push and Undo the last commit, then the Branches menu (switch, including to a remote branch, which makes the local one, or make a new one), Stash (put every change aside, new files too, and bring the last stash back) and More (stage all, unstage all, throw away all unstaged changes, read again).
- *The graph.* Every branch's commits, newest first, in lanes that never move, so a branch is a straight line; the working tree is the top row, joined to HEAD by a dashed line, with how many files are staged and not. Each commit shows the author's initials on a coloured gradient, or Claude Code's (CC) or Codex's (CX) when the agent wrote it or is a co-author, its message, how long ago, and its branches and tags (a globe when the remote has the branch). More commits load as you scroll. A click on a commit shows its whole message, who wrote it and the files it changed; a click on a file opens what the commit changed in it.
- *Staging and committing.* Under the graph, Staged and Unstaged files with their letter (M, A, D, R, ! for a conflict) and lines added and removed; hovering a file offers Open, Throw away (Delete for a new file, after asking) and Stage or Unstage. A click opens the file's diff as an editor tab, with old and new line numbers, which follows edits and can stage or unstage the file from its bar. A new file over 50 MB shows its size, and staging it asks first: "Ignore it" adds it to `.gitignore` (and stages that), since GitHub refuses files over 100 MB. The summary and description boxes take the message (⌘↩ commits); Amend changes the last commit, filling in its message; the Commit button's menu has Commit and push, and Stage all and commit.
- *Write it.* Asks Claude Code (its small model, no tools) or Codex for the message, from the staged changes and the recent subjects, and fills the boxes to edit. It runs the agent's own CLI as the user is signed in to it, like the Agents pane; no keys or tokens are read.
- *Stopped in a merge.* While git is merging or rebasing a banner says so with Stop (`merge --abort` and the like); conflicted files show with ! among the unstaged, to fix in the editor and stage.
- *The git program.* The daemon runs the command line `git` on its machine (`git_status`, `git_log`, `git_show`, `git_diff`, `git_run`, `git_write_message`), so it works the same over ssh, and nothing here asks for a password: pushing to a remote needs ssh keys or a credential helper that already work in a terminal, and a failure shows git's own message. The pane reads the status every 3 s while it shows and when the window comes to the front, and reads the log again only when a branch or tag moves. A repository without a name and email for commits gets a form that sets them globally; a folder that isn't a repository gets a button to start one.
- *Not yet.* Staging part of a file (hunks or lines); side-by-side diffs and syntax colours in them; diffs that understand notebooks (an `.ipynb` diff is its JSON); adding and changing remotes, tags, rebasing and merging from the pane (a terminal does these); and avatars from GitHub.

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
    .gitignore       # written by Sandhill, ignores outputs/
  analysis.qmd
  renv.lock / environment.yml / pyproject.toml ...
```

- `project.toml` holds only settings a collaborator should share: the R and Python interpreters (or "detect"), the working-directory rule, and env files to load.
- Personal state (open tabs, pane layout, recent commands) is **not** written into the folder. The daemon keeps it in its own state directory, keyed by the project path. RStudio puts this in `.Rproj.user/`, which people then have to gitignore; we avoid that.
- Any folder can be opened as a project. `.sandhill/` is created only when you first change a project setting, so opening a colleague's repo leaves no trace.

**What it does.**
1. **Open and switch.** "Open Project…" picks a folder on a host. A start screen lists recent projects per host (server and Mac separately). A window has a tab per machine, each showing one project; the project name at the top of the left pane switches the tab to another project on that machine (section 3, "The look").
2. **Restore.** Reopening a project brings back its tabs, file tree and layout, and reattaches to that project's kernels that are still running. The daemon tags each kernel session with its project, so sessions can be listed per project.
3. **Interpreter per project.** On first open the daemon looks for `renv.lock`, `environment.yml`, `pixi.toml`, `pyproject.toml`/`uv.lock` and `.venv/`, and proposes the matching R and Python. You confirm once and the choice is saved in `project.toml`. This is the "remembers a choice per project" line in the §5b table.
4. **Working directory.** The console and plain `.R`/`.py` scripts run in the project root, as in RStudio. Chunks in `.qmd`/`.Rmd`/`.ipynb` run in the document's own folder, because that is where `quarto render` runs them, so relative paths behave the same interactively and when rendering (RStudio's default for R Markdown too). `project.toml` can switch documents to the project root.
5. **Startup files.** R starts in the project root, so its `.Rprofile` and `.Renviron` there apply as usual; this is how renv activates. For Python, a `.env` file listed in `project.toml` is loaded into the kernel's environment.
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
- A project window reopens its tabs, and each document picks up the sessions that last ran its cells. Its kernel picker offers that project's sessions (and ones started outside any project), not other projects'.
- The folder and tabs the app remembered before projects come back once, as a project, the first time it is opened.

Not yet: `.sandhill/project.toml`, interpreter detection, the working-directory switch, `.env` loading, the output cache in `.sandhill/outputs/`, and agents (step 4).

---

## 6. Performance budget (targets, not measurements)

| Component | Target |
|---|---|
| Mac app, idle with 3 docs open | < 150 MB total (Tauri core + one WKWebView content process) |
| Cold start to editable | < 500 ms with outputs coming from the local cache, then sync with the server |
| Reopen a notebook with 200 outputs | Only the visible outputs are mounted; the rest are lazy |
| Server daemon | ~10–30 MB RSS. Kernels, LSPs and agents dominate, and they would exist in any IDE. |
| Per-keystroke work | All on the Mac (CodeMirror). Only LSP requests and executions cross ssh. |

Keep the daemon protocol binary (msgpack), stream outputs incrementally, and never send blobs the client already has (content hashes).

How notebooks stay small in the app (decided October 2026, after measuring three scanpy notebooks of 7, 7 and 25 MB): a `.ipynb` is mostly base64 plots, and every copy of that text costs memory in each process it passes through, which the allocator then keeps. So plots never travel as text. The daemon opens a notebook with `read_notebook`, which moves plots and outputs over 64 KB into the blob store and sends references; the web view loads them by URL. Saving sends the references back with `write_notebook`, and the daemon writes the values into the file. In the web view, images load lazily, far-away images are dropped until they scroll near, and off-screen outputs skip layout (`content-visibility`). After a large message the daemon and the app hand freed memory back to the system.

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
