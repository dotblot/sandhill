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
  - As built (step 2): the app ships `sandhilld` inside `Sandhill.app` and runs `sandhilld attach --start`, which starts the daemon in the background if it isn't running, so no launchd setup is needed. The remote case will run the same command through ssh. The launchd agent in `packaging/` stays optional, for starting the daemon at login.
- Keep the OS-specific parts behind one small trait from day one: service install (systemd vs launchd), job isolation (`systemd-run` scopes vs process groups), and the transport (ssh stdio vs Unix socket).
- Closing the app window doesn't stop local work, because the daemon keeps running. But work does stop while the Mac **sleeps or shuts down**. The daemon should hold a sleep assertion while jobs run (what `caffeinate` does), and warn before you close the lid with work in progress.
- The kernels (ipykernel, Ark) and agents (`claude`, `codex`) run natively on Apple silicon.
- Local mode is also the easiest way to develop and test the daemon, so build it first.

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
| **Interpreter discovery and switching** (conda/mamba, venv, uv, renv, several R versions) | v1 | The daemon scans the server and remembers a choice per project. |
| **MCP server for coding agents in live sessions** (Positron 2026.10, experimental) | MVP | Already the core of §5. Positron adding the same thing confirms the design. |
| **Assistant aware of your session** (variables, plots, errors) | v1 | The same MCP tools, plus "Fix" and "Explain" buttons on errors. |
| **Packages pane** | later | `pip`/`uv`/`conda`/`install.packages` wrappers. |
| **Run App** (Shiny, Streamlit, Dash…) | later | A job plus the Viewer pane. |
| **Data Connections pane** (databases, warehouses) | skip | Rarely needed for bioinformatics. |
| VS Code extension marketplace | **skip** | This is where most of Positron's memory goes. Build in what you need instead. |

### What code can be reused
- **Ark** (R kernel, LSP and the variables, data-explorer, plots and help comms) and **Air** (R formatter) are **MIT**. Use them freely. Ark alone gives the R side of most of the MVP panes.
- **Positron itself** and **Kallichore** (its Rust kernel supervisor, which resembles our daemon) are under the **Elastic License 2.0** ([licensing](https://positron.posit.co/licensing)). ELv2 allows personal and commercial use and derivative works. The main restriction is offering it to others as a hosted service. Copying ELv2 code puts that part of your project under those terms. Safer path: implement the same comm protocol (Ark's MIT code shows the message shapes) and write our side fresh. Use Kallichore as a design reference.

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
